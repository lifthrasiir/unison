//! The on-demand glyph items synthesized for names that are mentioned but not drawn.

use super::*;

/// Scan `all_items` for on-demand glyph names referenced in refs, maps,
/// and remaps. For each one not already defined as a glyph, append a
/// synthetic `DocumentItem::Glyph` (filled rectangle for WxH, or a
/// color/mono composite when X:mono and X:color both exist).
pub(super) fn inject_on_demand_glyph_items(
    all_items: &mut Vec<ExpandedItem>,
    map_targets: Vec<MapTarget>,
    name_parts: &NamePartsMap,
    aliases: &crate::alias::AliasMap,
    undecided_parts: &HashSet<(Option<ItemRef>, String)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut defined: HashSet<String> = HashSet::default();
    // Where each glyph's body is, not a copy of it: the only reader below is
    // the color/mono pair, which is a handful of glyphs, while a copy here is
    // every pixel grid in the font cloned for nothing.
    let mut glyph_bodies: HashMap<String, usize> = HashMap::default();
    // A glyph with neither a pixel grid nor a ref never enters the resolution
    // cache (see `glyph_cache::seed_cache`), so it is not built and every use
    // of it — cmap entry, composite component, GSUB coverage — is dropped. It
    // is as unusable as an undefined name, and reported the same way. The one
    // exception is a `keep` placeholder, which `seed_cache` does build as an
    // empty anchor-carrying entry (and `issues.rs` likewise exempts from its
    // "has no content" warning).
    let mut contentless: HashSet<String> = HashSet::default();

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
    let mut mention_seen: HashSet<(Option<ItemRef>, String)> = HashSet::default();
    let mut consider = |name: &str, origin: Option<ItemRef>, by: Option<&str>, kind: RefKind| {
        let unusable = !defined.contains(name) || contentless.contains(name);
        if unusable && mention_seen.insert((origin, name.to_string())) {
            mentions.push(Mention {
                name: name.to_string(),
                origin,
                by: by.map(str::to_string),
                kind,
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
                    consider(&r.name, e.origin, Some(by), RefKind::Ref);
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
                        consider(&name, e.origin, None, RefKind::Remap);
                    }
                }
            }
            _ => {}
        }
    }
    // Map targets were expanded once by the caller: `glyph` is still a pattern
    // on the item, and the cmap builder expands it per codepoint.
    for t in map_targets {
        consider(&t.name, t.origin, None, RefKind::Map(t.char_repr));
    }

    // Synthesis is per unique name; reporting is per mention, so the loops
    // are separate.
    let unique: Vec<(String, Option<ItemRef>)> = {
        let mut seen: HashSet<&str> = HashSet::default();
        mentions
            .iter()
            // Contentless names are in `mentions` to be reported, but they are
            // defined, so there is nothing to synthesize for them.
            .filter(|m| !defined.contains(&m.name) && seen.insert(m.name.as_str()))
            .map(|m| (m.name.clone(), m.origin))
            .collect()
    };
    let mut unresolved: HashSet<String> = HashSet::default();

    // The alias items are gone from `all_items` by now, so `defined` holds
    // only the glyphs themselves. A half of a color/mono pair may well be
    // stated as an alias (`glyph X:color = Y:color`) — it is a second name for
    // a glyph, so the pair is complete — and both the existence check and the
    // body lookup have to see through it.
    fn canonical<'a>(aliases: &'a crate::alias::AliasMap, n: &'a str) -> &'a str {
        aliases.resolved_target(n).unwrap_or(n)
    }

    // Recognizing a name and drawing the shape it asks for read nothing but
    // the name and `defined`, and a shape is exact geometry — a cold build
    // draws hundreds of them — so both run on every core ahead of the walk,
    // which then only appends in order.
    use crate::on_demand::{OnDemandGlyph, detect_on_demand_glyph};
    let synthesized =
        crate::parallel::map_indexed(unique.len(), &crate::cancel::CancelToken::never(), |i| {
            let found =
                detect_on_demand_glyph(&unique[i].0, |n| defined.contains(canonical(aliases, n)));
            let grid = match &found {
                Some(OnDemandGlyph::Shape(spec)) => {
                    Some(crate::on_demand::make_on_demand_grid(spec))
                }
                _ => None,
            };
            (found, grid)
        });
    for ((name, origin), slot) in unique.into_iter().zip(synthesized) {
        let (found, grid) = slot.expect("a `never` token cannot cancel");
        match found {
            Some(OnDemandGlyph::Shape(spec)) => {
                let grid = grid.expect("drawn above for every shape");
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
                    // The finer of the two lattices both halves land on; one
                    // past what a `scale` can state has no glyph to go into.
                    let Some(combined_scale) =
                        crate::math::lcm_u64(u64::from(mono_s), u64::from(color_s))
                            .and_then(|lcm| u8::try_from(lcm).ok())
                    else {
                        diagnostics.push(Diagnostic::error(
                            origin,
                            format!(
                                "color/mono glyph '{name}' cannot be synthesized: \
                                 scales {mono_s} and {color_s} have no common scale up to 255",
                            ),
                        ));
                        continue;
                    };
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
                            goto: r.goto,
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
                            goto: r.goto,
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

                    // `vectoronly` does not travel with the grid the way
                    // `desync` does: it describes the *drawing*, not what one
                    // grid is for. But the merged glyph is *two* drawings, and
                    // a flag written on one half must not reach the other
                    // half's components — see `vectoronly_layers`.
                    let vectoronly = mono_body.vectoronly || color_body.vectoronly;
                    let vectoronly_layers = match (mono_body.vectoronly, color_body.vectoronly) {
                        (true, false) => Some(LayerVisibility::MonoOnly),
                        (false, true) => Some(LayerVisibility::ColorOnly),
                        _ => None,
                    };

                    all_items.push(ExpandedItem {
                        item: DocumentItem::Glyph {
                            name: GlyphName(name),
                            body: GlyphBody {
                                refs,
                                points,
                                pixels,
                                desync,
                                vectoronly,
                                vectoronly_layers,
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
    } in mentions
    {
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
}
