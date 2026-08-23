//! The codepoint side of the source: what every `map` line claims, the
//! duplicates and cross-slice conflicts that fall out of it, and the
//! variation-sequence checks that go with it. Also the rest of the
//! per-item scan — features, `name-parts` values, headings and unrecognized
//! directives — which walks the same items.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use crate::document::{
    Directive, DocumentItem, MAX_HEADING_LEVEL, classify_directive, substitute_name_parts,
};

use super::{Cx, Issue, Severity, issue_at, short_path};

/// Every codepoint a `map` claims, checked for duplicates within a slice
/// and for conflicts between two slices one face includes; returns the
/// glyph names the maps *use*, which is what the unused-glyph pass roots
/// its reachability at.
/// Where one codepoint was mapped: the file, the `DocLine` index and the
/// 1-based file line, as [`Issue`] wants them.
struct MapSite {
    file: PathBuf,
    line: usize,
    file_line: usize,
}

pub(super) fn check_maps(cx: &Cx<'_>, issues: &mut Vec<Issue>) -> HashSet<String> {
    let docs = cx.docs;
    let name_parts = cx.name_parts;
    let scoped_parts = &cx.scoped_parts;
    let faces = cx.faces;
    let groups = &cx.groups;
    let _resolution = cx.resolution;
    // Every codepoint, by the slice that maps it. Two entries in one slice are
    // the duplicate this has always warned about; two entries in *different*
    // slices are only a problem for a face that includes both, which is the
    // conflict the face split exists to make explicit.
    let mut mapped_codepoints: HashMap<u32, BTreeMap<Option<String>, MapSite>> = HashMap::new();
    let mut mapped_glyphs: HashSet<String> = HashSet::new();

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in cx.source_items(doc_idx) {
            // The empty target (`` `` ``) says that a character none of the
            // other alternatives covers is not an error — so it always matches,
            // and anything written after it can never be reached. Read as
            // written, since this is about the line and not about what it
            // expands to. See `resolve_map_alternatives`.
            if let DocumentItem::Map { glyphs, .. } = item
                && glyphs.iter().rev().skip(1).any(String::is_empty)
            {
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    "the empty `` target has to be the last one: it always matches, so every \
                     alternative after it is unreachable"
                        .to_string(),
                ));
            }
            match item {
                // A variation sequence maps no codepoint on its own, so it
                // neither duplicates nor conflicts with a plain mapping of the
                // same base — but its target is every bit as *used* as a plain
                // map's, and the font does contain that glyph.
                DocumentItem::Map {
                    slices,
                    char_repr,
                    selector: Some(sel),
                    glyphs,
                    ..
                } => {
                    let stated: Vec<Option<String>> = if slices.is_empty() {
                        vec![None]
                    } else {
                        slices.iter().cloned().map(Some).collect()
                    };
                    // Every alternative counts as used, not just the one the
                    // build ends up picking: a fallback nothing reached today
                    // is still named on purpose, and "unused glyph" would ask
                    // the author to delete the safety net.
                    for slice in stated {
                        for glyph in glyphs {
                            let subst_glyph = substitute_name_parts(
                                glyph,
                                scoped_parts.for_slice(slice.as_deref()),
                            );
                            if let Ok(triples) = crate::render::ttf_builder::expand_uvs_map_triples(
                                char_repr,
                                sel,
                                &subst_glyph,
                            ) {
                                mapped_glyphs.extend(triples.into_iter().map(|(_, _, name)| name));
                            }
                        }
                    }
                }
                // Unresolvable refs, map targets and remap operands are all
                // reported by the resolution pass above.
                DocumentItem::Map {
                    slices,
                    char_repr,
                    glyphs,
                    ..
                } => {
                    // Once per slice the line is stated for, with that slice's
                    // name parts — exactly as the build expands it.
                    let stated: Vec<Option<String>> = if slices.is_empty() {
                        vec![None]
                    } else {
                        slices.iter().cloned().map(Some).collect()
                    };
                    for slice in stated {
                        let parts = scoped_parts.for_slice(slice.as_deref());
                        // Which codepoints the line claims is what the
                        // duplicate scan is about, and every alternative
                        // expands over the same ones — so they are walked once,
                        // over the first, while *every* alternative counts as a
                        // used glyph name (see the variation-sequence arm).
                        // Expanded together rather than one alternative at a
                        // time: the character spec is the same for all of them,
                        // and a range line is thousands of characters wide.
                        let substituted: Vec<String> = glyphs
                            .iter()
                            .map(|g| substitute_name_parts(g, parts))
                            .collect();
                        let per_alt = crate::render::ttf_builder::expand_map_pairs_per_alternative(
                            char_repr,
                            &substituted,
                        );
                        for alt in &per_alt[1..] {
                            mapped_glyphs.extend(alt.iter().map(|(_, name)| name.clone()));
                        }
                        for (cp, target) in &per_alt[0] {
                            mapped_glyphs.insert(target.clone());
                            let by_slice = mapped_codepoints.entry(*cp).or_default();
                            if let Some(prev) = by_slice.get(&slice) {
                                issues.push(issue_at(
                                    doc,
                                    item_idx,
                                    Severity::Warning,
                                    format!(
                                        "duplicate codepoint mapping U+{:04X} (first at {}:{})",
                                        cp,
                                        short_path(&prev.file),
                                        prev.file_line,
                                    ),
                                ));
                            } else {
                                let (line, file_line) = doc.item_lines(item_idx);
                                by_slice.insert(
                                    slice.clone(),
                                    MapSite {
                                        file: doc.path.clone(),
                                        line,
                                        file_line,
                                    },
                                );
                            }
                        }
                    }
                }
                DocumentItem::Feature {
                    scripts,
                    remap_group,
                    ..
                } => {
                    if !groups.info.contains_key(remap_group.as_str()) {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("feature references undefined remap group '{}'", remap_group,),
                        ));
                    }
                    for issue in script_lang_issues(scripts) {
                        issues.push(issue_at(doc, item_idx, Severity::Error, issue));
                    }
                }
                DocumentItem::FeatureAnchor { scripts, .. } => {
                    for issue in script_lang_issues(scripts) {
                        issues.push(issue_at(doc, item_idx, Severity::Error, issue));
                    }
                }
                DocumentItem::NameParts { name, values, .. } => {
                    for val in values {
                        if val.starts_with('$') && !name_parts.contains_key(val.as_str()) {
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Warning,
                                format!("undefined name-parts reference '{}'", val,),
                            ));
                        }
                    }
                    // A value is a pattern, so a binding can fail to expand on
                    // its own — before any glyph line refers to it.
                    if let Err(msg) =
                        crate::document::try_resolve_name_part_values(values, name_parts)
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("name part `{name}`: {msg}"),
                        ));
                    }
                }
                DocumentItem::Heading { level, .. } if *level > MAX_HEADING_LEVEL => {
                    // A heading builds nothing, so this could have been a
                    // warning — but a `####` line is one the author meant as a
                    // section and the editor will not group, and silently
                    // dropping structure is what the error is for.
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "heading level {level} is past `{}`, the deepest this format has",
                            "#".repeat(MAX_HEADING_LEVEL as usize),
                        ),
                    ));
                }
                DocumentItem::Directive(text) => {
                    // `font-meta` became `meta`, one key per line. Named
                    // separately from the generic unrecognized-directive
                    // error below, which would leave the author rereading a
                    // line that is spelled correctly for the format it was
                    // written against.
                    if text.trim_start().starts_with("font-meta") {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "`font-meta` was replaced by `meta`, one key per line \
                             (`{}` becomes {})",
                                text.trim(),
                                legacy_font_meta_replacement(text),
                            ),
                        ));
                    } else if classify_directive(text) == Directive::Unrecognized {
                        // An error rather than a warning because of the one
                        // way this line is usually written: a pixel row whose
                        // width does not match its glyph header parses as a
                        // directive, the row is dropped, and the glyph builds
                        // blank or half-drawn — with a `map` still pointing a
                        // character at it. The font builds through warnings,
                        // so a warning here is a glyph silently going missing.
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("unrecognized directive '{}'", text.trim(),),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // Two slices of one face mapping the same character is the conflict the
    // face split exists to surface. There is no override rule to fall back on
    // — see `crate::faces` — so it is an error naming the face that reaches
    // both, and the fix is to move the character out of whichever slice should
    // not have had it.
    //
    // Sorted by codepoint: the report is a golden, and a HashMap would make its
    // order depend on the hasher.
    let mut conflicts: Vec<(u32, &BTreeMap<Option<String>, MapSite>)> = mapped_codepoints
        .iter()
        .filter(|(_, by_slice)| by_slice.len() > 1)
        .map(|(cp, by_slice)| (*cp, by_slice))
        .collect();
    conflicts.sort_by_key(|(cp, _)| *cp);
    for (cp, by_slice) in conflicts {
        for face in &faces.faces {
            let present: Vec<(&Option<String>, &MapSite)> = by_slice
                .iter()
                .filter(|(slice, _)| face.includes(slice.as_deref()))
                .collect();
            if present.len() < 2 {
                continue;
            }
            let describe = |slice: &Option<String>| match slice {
                Some(s) => format!("slice `{s}`"),
                None => "the base slice".to_string(),
            };
            // Report against the later declaration, so the first one reads as
            // the definition and the rest as the intrusions.
            let (first_slice, first) = present[0];
            for (slice, site) in &present[1..] {
                issues.push(Issue {
                    severity: Severity::Error,
                    glyph: None,
                    message: format!(
                        "U+{cp:04X} is mapped in both {} and {}, and face `{}` includes both \
                         (first at {}:{})",
                        describe(first_slice),
                        describe(slice),
                        face.label(),
                        short_path(&first.file),
                        first.file_line,
                    ),
                    file: site.file.clone(),
                    line: site.line,
                    file_line: site.file_line,
                });
            }
        }
    }

    mapped_glyphs
}

/// Spell out the `meta` lines a legacy `font-meta` line becomes, so the error
/// is something to paste rather than something to look up. Falls back to the
/// bare keyword when the old line is too malformed to split into pairs.
fn legacy_font_meta_replacement(text: &str) -> String {
    let Ok(tokens) = crate::document_io::tokenize_tokens(text) else {
        return "`meta KEY VALUE`".to_string();
    };
    let pairs: Vec<String> = tokens[1..]
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| format!("`meta {} {}`", c[0], c[1]))
        .collect();
    if pairs.is_empty() {
        "`meta KEY VALUE`".to_string()
    } else {
        pairs.join(" + ")
    }
}

fn script_lang_issues(targets: &[String]) -> Vec<String> {
    let mut issues = Vec::new();
    for target in targets {
        let mut parts = target.split('/');
        let script = parts.next().unwrap_or("");
        let lang = parts.next();
        if parts.next().is_some() {
            issues.push(format!(
                "feature target '{target}' has more than one '/'; \
                 write it as SCRIPT or SCRIPT/LANGUAGE",
            ));
            continue;
        }
        for (what, tag) in [("script", Some(script)), ("language", lang)] {
            let Some(tag) = tag else { continue };
            if tag.is_empty() || tag.len() > 4 || !tag.is_ascii() {
                issues.push(format!(
                    "feature target '{target}' has an invalid {what} tag '{tag}'; \
                     OpenType tags are 1 to 4 ASCII characters",
                ));
            }
        }
    }
    issues
}

/// The two variation-sequence problems that are only visible once names are
/// resolved, so they are read off the expansion rather than the raw documents:
/// it has already substituted name parts and dropped the slices a face does not
/// include, which is the form the builder itself sees.
///
/// Both are about the *fallback* lookup, which is keyed by glyph id where cmap
/// format 14 is keyed by codepoint. Wherever two codepoints share a base glyph
/// the two halves of one declaration stop agreeing, and that gap is what these
/// report.
pub(super) fn uvs_collision_diagnostics(
    expansion: &crate::render::ttf_builder::Expansion,
) -> Vec<crate::resolve::Diagnostic> {
    use crate::render::ttf_builder::{expand_map_pairs, expand_uvs_map_triples};

    let mut out = Vec::new();

    // Which glyph each codepoint reaches, and which codepoints reach each glyph.
    let mut cp_to_glyph: HashMap<u32, String> = HashMap::new();
    let mut glyph_to_cps: HashMap<String, Vec<u32>> = HashMap::new();
    for e in &expansion.items {
        let DocumentItem::Map {
            char_repr,
            selector: None,
            glyphs,
            ..
        } = &e.item
        else {
            continue;
        };
        let glyph = crate::render::ttf_builder::resolved_map_target(glyphs);
        for (cp, name) in expand_map_pairs(char_repr, glyph) {
            glyph_to_cps.entry(name.clone()).or_default().push(cp);
            cp_to_glyph.insert(cp, name);
        }
    }

    // (base glyph, selector) → the target the first pair claimed.
    let mut claimed: HashMap<(String, u32), String> = HashMap::new();
    for e in &expansion.items {
        let DocumentItem::Map {
            char_repr,
            selector: Some(sel),
            glyphs,
            ..
        } = &e.item
        else {
            continue;
        };
        let glyph = crate::render::ttf_builder::resolved_map_target(glyphs);
        let Ok(triples) = expand_uvs_map_triples(char_repr, sel, glyph) else {
            continue;
        };
        for (base, selector, target) in triples {
            let Some(base_glyph) = cp_to_glyph.get(&base) else {
                // Reported as an unmapped base by `check_uvs_maps`.
                continue;
            };
            match claimed.get(&(base_glyph.clone(), selector)) {
                Some(first) if *first != target => out.push(crate::resolve::Diagnostic::error(
                    e.origin,
                    format!(
                        "map 'U+{base:04X} U+{selector:04X}' targets '{target}', but a pair on the \
                         same glyph '{base_glyph}' already targets '{first}'; the fallback lookup \
                         is keyed by glyph and can only hold one of them",
                    ),
                )),
                Some(_) => {}
                None => {
                    claimed.insert((base_glyph.clone(), selector), target);

                    // The same key seen from the other side: a base glyph that
                    // more than one character reaches makes the fallback rule
                    // fire for a sequence nobody declared. cmap 14 stays exact,
                    // so the two paths disagree.
                    let others: Vec<u32> = glyph_to_cps
                        .get(base_glyph)
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|cp| *cp != base)
                        .collect();
                    if !others.is_empty() {
                        let listed: Vec<String> =
                            others.iter().map(|cp| format!("U+{cp:04X}")).collect();
                        out.push(crate::resolve::Diagnostic::new(
                            Severity::Warning,
                            e.origin,
                            format!(
                                "map 'U+{base:04X} U+{selector:04X}': glyph '{base_glyph}' is also \
                                 reached by {}, so the fallback lookup applies this pair to {} too \
                                 — cmap format 14 will not",
                                listed.join(", "),
                                if others.len() == 1 { "it" } else { "them" },
                            ),
                        ));
                    }
                }
            }
        }
    }

    out
}

/// Spell a codepoint list out, because the characters themselves cannot be
/// read: a variation selector is invisible, which is the whole reason these
/// messages exist.
fn spell_codepoints(s: &str) -> String {
    s.chars()
        .map(|c| format!("U+{:04X}", c as u32))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `map BASE SELECTOR = GLYPH`, and the plain `map` shapes that only ever occur
/// because someone meant to write one.
///
/// A separate pass rather than an arm in the main item loop, because the rule
/// that matters most — the base has to be mapped too — can only be judged once
/// every plain `map` in every document has been seen, and a source is free to
/// state the pair before the base.
pub(super) fn check_uvs_maps(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    use crate::render::ttf_builder::{
        UvsExpandError, expand_map_codepoints, expand_uvs_map_triples,
    };
    use crate::ucd::is_variation_selector;

    // Which codepoints a plain `map` claims, per slice. `None` is the base
    // slice, which every face includes.
    //
    // Both loops walk `Cx::source_items` rather than the raw items, because a
    // `map` an `exists` governs still says `U+($1)` on the line: read as
    // written it names no codepoint at all, and a pair whose base is stated by
    // another scoped `map` would read as a base mapped nowhere.
    let mut base_cps: HashMap<Option<&str>, HashSet<u32>> = HashMap::new();
    for doc_idx in 0..docs.len() {
        for (_, item) in cx.source_items(doc_idx) {
            let DocumentItem::Map {
                slices,
                char_repr,
                selector: None,
                ..
            } = item
            else {
                continue;
            };
            let cps = expand_map_codepoints(char_repr);
            if slices.is_empty() {
                base_cps.entry(None).or_default().extend(cps);
            } else {
                for s in slices {
                    base_cps
                        .entry(Some(s.as_str()))
                        .or_default()
                        .extend(cps.iter().copied());
                }
            }
        }
    }

    // Lenient on purpose where a face's slice set would decide it: a pair
    // stated for the base slice is satisfied by a base mapped in *any* slice,
    // since `faces.rs` already forbids a character whose mapping varies from
    // being in the base at all — every face then has exactly one of them. The
    // check still catches the real mistake, which is a base mapped nowhere or
    // only in a slice this pair can never meet.
    let satisfied = |cp: u32, slice: Option<&str>| -> bool {
        if base_cps.get(&None).is_some_and(|s| s.contains(&cp)) {
            return true;
        }
        match slice {
            Some(s) => base_cps.get(&Some(s)).is_some_and(|set| set.contains(&cp)),
            None => base_cps.values().any(|set| set.contains(&cp)),
        }
    };

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in cx.source_items(doc_idx) {
            let DocumentItem::Map {
                slices,
                char_repr,
                selector,
                ..
            } = item
            else {
                continue;
            };
            let stated: Vec<Option<&str>> = if slices.is_empty() {
                vec![None]
            } else {
                slices.iter().map(|s| Some(s.as_str())).collect()
            };

            let Some(sel) = selector else {
                // A plain `map` that names a selector. Two shapes, and each
                // gets its own message because the fixes are different.
                if char_repr.chars().count() > 1
                    && char_repr.chars().any(|c| is_variation_selector(c as u32))
                {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "map names the {}-character sequence {}; cmap format 14 holds a base \
                             and one selector and nothing longer — map the first two and put the \
                             rest in a `remap`",
                            char_repr.chars().count(),
                            spell_codepoints(char_repr),
                        ),
                    ));
                } else if let Some(cp) = expand_map_codepoints(char_repr)
                    .into_iter()
                    .find(|cp| is_variation_selector(*cp))
                {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "map U+{cp:04X}: a variation selector is reachable only as the second \
                             half of a `map BASE SELECTOR` pair, whose glyph the build owns",
                        ),
                    ));
                }
                continue;
            };

            match expand_uvs_map_triples(char_repr, sel, "") {
                Err(UvsExpandError::BothVary) => issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    format!(
                        "map '{char_repr} {sel}': only one half of a variation sequence may vary \
                         — the other has to name a single codepoint",
                    ),
                )),
                Err(UvsExpandError::Empty { selector_half }) => issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    format!(
                        "map '{}' names no valid codepoint",
                        if selector_half { sel } else { char_repr },
                    ),
                )),
                Err(UvsExpandError::NotASelector { cp, selector_half }) => issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    if selector_half {
                        format!(
                            "map '{char_repr} {sel}': U+{cp:04X} is not a variation selector, so \
                             nothing would ever shape this pair",
                        )
                    } else {
                        format!(
                            "map '{char_repr} {sel}': U+{cp:04X} is a variation selector, not a \
                             base character — the halves are the wrong way round",
                        )
                    },
                )),
                Ok(triples) => {
                    for slice in &stated {
                        for (base, _, _) in &triples {
                            if satisfied(*base, *slice) {
                                continue;
                            }
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Error,
                                match slice {
                                    Some(s) => format!(
                                        "map '{char_repr} {sel}': base U+{base:04X} is not mapped \
                                         in slice '{s}', so the fallback lookup has no first glyph",
                                    ),
                                    None => format!(
                                        "map '{char_repr} {sel}': base U+{base:04X} is not mapped, \
                                         so the fallback lookup has no first glyph",
                                    ),
                                },
                            ));
                            break;
                        }
                    }
                }
            }
        }
    }
}
