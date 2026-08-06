//! Cross-document validation: everything that can only be judged with all
//! `.unf` files in hand.
//!
//! Missing and dangling refs, duplicate `map`s, unused glyphs, remap sanity.
//! Resolution itself emits its diagnostics directly (see [`crate::resolve`]);
//! this module is for what no single item's resolution can see. Both the build
//! and the editor print the same report, `error:`/`warning:` prefixed and
//! `file:line:` located, and a font with only warnings still builds — so the
//! report is meant to be read, not just exit-coded.
//!
//! A few rules are worth knowing about because they are refusals rather than
//! best-effort output:
//!
//! - a `remap` whose source and target lists are N→M or N→0 has no OpenType
//!   lookup type at all, so it is an error here instead of something the builder
//!   emits close-but-wrong;
//! - referring to a contentless glyph — one with no pixel grid and no `ref`, see
//!   [`crate::document_io`] — from a `map`, `ref` or `remap` is an error, since
//!   such a glyph never enters the resolution cache;
//! - the two anchor-exposure ambiguities in [`crate::ref_composite`] are errors,
//!   reported through an anchors-only resolution pass.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::document::{
    Directive, Document, DocumentItem, GlyphName, classify_directive, expand_name_element,
    find_invalid_inline_ranges, is_name_pattern, is_valid_glyph_name, substitute_name_parts,
};
use crate::pattern::NamePattern;
use crate::resolve::{Diagnostic, DocSet, ItemRef, Resolution};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
    pub file: PathBuf,
    /// DocLine index (0-based), used for editor navigation.
    pub line: usize,
    /// 1-based file line number, used for display.
    pub file_line: usize,
}

/// An issue anchored at item `item_idx`'s defining line in `doc`.
fn issue_at(doc: &Document, item_idx: usize, severity: Severity, message: String) -> Issue {
    let (line, file_line) = doc.item_lines(item_idx);
    Issue {
        severity,
        message,
        file: doc.path.clone(),
        line,
        file_line,
    }
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

/// Problems in a `feature ... for ...` target list.
///
/// A target is an OpenType script tag, optionally narrowed to one language
/// system below it as `script/LANG`. Both registries use 4-byte tags, so a
/// longer part would be silently truncated to something that resolves to
/// nothing — worth an error rather than a font that quietly ignores the
/// declaration.
/// The first `$part` in `name` that `parts` does not bind but some slice does.
///
/// A slice-scoped binding substitutes nothing where it does not apply, and what
/// is left behind — a `$` in a glyph name, a `ref` to nothing — says nothing
/// about why. This is what lets the report say it.
fn unbound_scoped_part(
    name: &str,
    parts: &crate::pattern::NamePartsMap,
    scoped: &crate::document::SliceNameParts,
) -> Option<String> {
    if !name.contains('$') {
        return None;
    }
    name.match_indices('$').find_map(|(at, _)| {
        let end = name[at + 1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .map_or(name.len(), |i| at + 1 + i);
        let part = &name[at..end];
        (!parts.contains_key(part) && scoped.is_slice_scoped(part)).then(|| part.to_string())
    })
}

/// The names written on one item, for a check that works on the source text
/// rather than on what expansion made of it.
fn written_names(item: &DocumentItem) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    match item {
        DocumentItem::Glyph { name, body } => {
            names.push(name.0.as_str());
            names.extend(body.refs.iter().map(|r| r.name.as_str()));
        }
        DocumentItem::GlyphAlias { name, target, .. } => {
            names.push(name.0.as_str());
            names.push(target.as_str());
        }
        DocumentItem::Map { glyph, .. } => names.push(glyph.as_str()),
        DocumentItem::MapDecomposed { glyph, .. } => {
            names.extend(glyph.as_deref());
        }
        DocumentItem::Remap { .. } => names.extend(item.remap_operands().map(String::as_str)),
        _ => {}
    }
    names
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

pub fn collect_issues(docs: &[&Document]) -> Vec<Issue> {
    collect_issues_with(docs, &Resolution::compute(docs))
}

/// Validate `docs` against an already-computed [`Resolution`].
///
/// Callers that resolve for their own reasons — the editor's glyph cache, the
/// font build — should use this rather than [`collect_issues`], which resolves
/// again from scratch.
pub fn collect_issues_with(docs: &[&Document], resolution: &Resolution) -> Vec<Issue> {
    let mut issues = Vec::new();

    let name_parts = &resolution.name_parts;
    // Validation expands a slice-qualified line the way the build does: once
    // per slice, with that slice's bindings.
    let scoped_parts = crate::document::SliceNameParts::with_base(docs, name_parts.clone());
    let expansion = &resolution.expansion;
    let docset = DocSet::new(docs);

    // The face/slice graph: bad ids, cycles and undeclared slices reached from
    // a `face` line are reported by `FaceSet` itself.
    let faces = &resolution.faces;
    issues.extend(docset.to_issues(&faces.diagnostics));

    // Resolution is the same expansion the font build performs, so the
    // problems it detects — unresolvable references, maps that cannot be
    // synthesized, on-demand names that resolve to nothing — are reported
    // here instead of silently skipped, and this file does not reimplement
    // any of it.
    issues.extend(docset.to_issues(&expansion.diagnostics));

    // Duplicate alias declarations and alias cycles; see `crate::alias`.
    let aliases = &expansion.aliases;
    issues.extend(docset.to_issues(&aliases.diagnostics));

    // Every glyph the font will actually contain, including synthesized
    // on-demand and decomposed-map glyphs. Aliases are deliberately absent:
    // they are names, not glyphs, and the expansion has already rewritten
    // every reference to them.
    let all_glyph_names: HashSet<String> = expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Glyph {
                name: GlyphName(n), ..
            } => Some(n.clone()),
            _ => None,
        })
        .collect();

    // What an alias names has to exist, and the name has to be free. Neither
    // is visible to the alias collection itself, which runs before the glyph
    // set is known.
    for decl in aliases.decls() {
        if all_glyph_names.contains(&decl.name) {
            issues.push(docset.to_issue(&Diagnostic::error(
                decl.origin,
                format!(
                    "`{}` is both a glyph and an alias; an alias is another name for a glyph, \
                     not a second definition of one",
                    decl.name,
                ),
            )));
        }
        // `resolved_target` is `None` for an alias in a cycle, which has
        // already been reported and would only produce a second, misleading
        // complaint about a target that is really itself. An on-demand name
        // (`a8x16`, `x:color`) is only synthesized where it is referenced, so
        // an unreferenced alias to one is not a missing glyph.
        let Some(target) = aliases.resolved_target(&decl.name) else {
            continue;
        };
        if !all_glyph_names.contains(target)
            && crate::ref_composite::parse_on_demand_glyph(target).is_none()
        {
            issues.push(docset.to_issue(&Diagnostic::error(
                decl.origin,
                format!(
                    "glyph alias `{}` names undefined glyph `{target}`",
                    decl.name
                ),
            )));
        }
    }

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let referenced: Vec<&String> = match item {
                DocumentItem::AssertShape { slices, .. } => slices.iter().collect(),
                DocumentItem::Slice { inherits, .. } => inherits.iter().collect(),
                item => item.slice_qualifier().iter().collect(),
            };
            for name in referenced {
                if !faces.declared.contains_key(name) {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!("undeclared slice `{name}`",),
                    ));
                }
            }
            // The same slice twice in one qualifier would state the line twice
            // for it, which is a duplicate mapping and never what was meant.
            let qualifier = item.slice_qualifier();
            for (i, name) in qualifier.iter().enumerate() {
                if qualifier[..i].contains(name) {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!("slice `{name}` is listed twice in this qualifier"),
                    ));
                }
            }
            // A name part bound per slice reaches only the lines stated for
            // that slice. Checked against what was written, because what it
            // fails as downstream — an odd glyph name, a `ref` to nothing —
            // never mentions the binding.
            {
                let stated: Vec<Option<&str>> = if qualifier.is_empty() {
                    vec![None]
                } else {
                    qualifier.iter().map(|s| Some(s.as_str())).collect()
                };
                let mut reported: Vec<String> = Vec::new();
                for slice in stated {
                    let parts = scoped_parts.for_slice(slice);
                    for name in written_names(item) {
                        let Some(part) = unbound_scoped_part(name, parts, &scoped_parts) else {
                            continue;
                        };
                        if reported.contains(&part) {
                            continue;
                        }
                        let where_ = match slice {
                            Some(s) => format!("slice `{s}`"),
                            None => "the base slice".to_string(),
                        };
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "name part `{part}` is bound per slice and not for {where_}, \
                                 so it substitutes nothing here",
                            ),
                        ));
                        reported.push(part);
                    }
                }
            }
            // A slice-scoped binding stands for one name part in one slice, so
            // it is one value. A list would go back to being an alternation
            // that the slices no longer control.
            if let DocumentItem::NameParts { slices, values, .. } = item
                && !slices.is_empty()
            {
                let resolved = crate::document::resolve_name_part_values(values, name_parts);
                if resolved.len() != 1 {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!(
                            "a slice-scoped `name-parts` takes exactly one value, not {}",
                            resolved.len(),
                        ),
                    ));
                }
            }
            // An assertion whose slice combination no face satisfies would
            // never run, and a test that silently does not run is worse than
            // one that fails.
            if let DocumentItem::AssertShape { slices, .. } = item
                && !slices.is_empty()
                && faces.declared.keys().any(|k| slices.contains(k))
                && !faces.faces.iter().any(|f| f.includes_all(slices))
            {
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Error,
                    format!(
                        "no face includes all of `{}`, so this assertion would never run",
                        slices.join("`, `"),
                    ),
                ));
            }
        }
    }

    // A name part is bound unqualified or per slice, never both: an
    // unqualified binding that a slice overrode would be a precedence rule,
    // and `crate::faces` has none. Two bindings for one slice are the same
    // conflict a slice deeper in.
    {
        let mut seen: HashMap<(&str, Option<&str>), ()> = HashMap::new();
        for doc in docs {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let DocumentItem::NameParts { slices, name, .. } = item else {
                    continue;
                };
                let stated: Vec<Option<&str>> = if slices.is_empty() {
                    vec![None]
                } else {
                    slices.iter().map(|s| Some(s.as_str())).collect()
                };
                for slice in stated {
                    if seen.insert((name.as_str(), slice), ()).is_some() {
                        let where_ = match slice {
                            Some(s) => format!("slice `{s}`"),
                            None => "no slice".to_string(),
                        };
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("name part `{name}` is already bound for {where_}"),
                        ));
                    }
                    // The unqualified binding is the one that would be
                    // overridden, so it is what the conflict is reported
                    // against — whichever line came second.
                    let other = if slice.is_none() {
                        seen.keys().any(|(n, s)| n == &name.as_str() && s.is_some())
                    } else {
                        seen.contains_key(&(name.as_str(), None))
                    };
                    if other {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "name part `{name}` is bound both unqualified and per slice; \
                                 a slice-scoped binding is not an override",
                            ),
                        ));
                    }
                }
            }
        }
    }

    // A slice nothing is qualified to gives every face that includes it
    // nothing. Mirrors the "remap group is declared but has no rules" warning,
    // and matters most mid-migration: moving characters out of the base into
    // two slices is exactly where a typo leaves one of them empty.
    //
    // Content is counted transitively, so a slice that exists only to compose
    // others (`slice both = narrow wide`) is not empty when they are not.
    {
        let mut has_own: HashSet<&str> = HashSet::new();
        for doc in docs {
            for item in &doc.items {
                match item {
                    // `name-parts` is deliberately not counted: a binding is
                    // how a slice spells a name, not something a face gets.
                    DocumentItem::Map { slices, .. }
                    | DocumentItem::MapDecomposed { slices, .. }
                    | DocumentItem::Feature { slices, .. }
                    | DocumentItem::FeatureAnchor { slices, .. } => {
                        has_own.extend(slices.iter().map(String::as_str));
                    }
                    DocumentItem::AssertShape { slices, .. } => {
                        has_own.extend(slices.iter().map(String::as_str));
                    }
                    _ => {}
                }
            }
        }
        // Reachability over `inherits`, bounded by the number of slices, so a
        // cycle (reported elsewhere) cannot spin here.
        for (name, (_, origin)) in &faces.declared {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut stack = vec![name.as_str()];
            let mut found = false;
            while let Some(cur) = stack.pop() {
                if !seen.insert(cur) {
                    continue;
                }
                if has_own.contains(cur) {
                    found = true;
                    break;
                }
                if let Some((inherits, _)) = faces.declared.get(cur) {
                    stack.extend(inherits.iter().map(String::as_str));
                }
            }
            if !found {
                issues.push(docset.to_issue(&Diagnostic::new(
                    Severity::Warning,
                    Some(*origin),
                    format!(
                        "slice `{name}` is declared but nothing is qualified to it, \
                         so every face including it gets nothing from it",
                    ),
                )));
            }
        }
    }

    // Names the font will actually carry, checked once against the charset.
    // Against the *expanded* names, so a pattern that produced something odd is
    // caught the same way a hand-written name would be.
    {
        let mut bad: Vec<(&String, Option<ItemRef>)> = expansion
            .items
            .iter()
            .filter_map(|e| match &e.item {
                DocumentItem::Glyph {
                    name: GlyphName(n), ..
                } if !is_valid_glyph_name(n) => Some((n, e.origin)),
                _ => None,
            })
            .collect();
        // One report per name, at the line that defined it. A pattern that
        // expands to many bad names would otherwise bury the file.
        bad.sort_by(|a, b| a.0.cmp(b.0));
        bad.dedup_by(|a, b| a.0 == b.0);
        for (name, origin) in bad {
            issues.push(docset.to_issue(&Diagnostic::error(
                origin,
                format!(
                    "glyph name `{name}` may only contain letters, digits, `-`, `.`, `_` and `:`",
                ),
            )));
        }
    }

    let mut glyph_defs: HashMap<String, (PathBuf, usize)> = HashMap::new();

    // Groups are collected up front rather than as the scan reaches them: a
    // `feature` line may precede every rule of the group it attaches, and a
    // declaration may follow them.
    let groups = crate::document::remap_group_order(docs);
    let mut remap_group_issues: Vec<(String, Severity, String)> = Vec::new();
    for (group, target) in &groups.unknown_after {
        remap_group_issues.push((
            group.clone(),
            Severity::Error,
            format!(
                "remap group '{}' is ordered after undefined group '{}'",
                group, target,
            ),
        ));
    }
    if !groups.cycle.is_empty() {
        let names = groups.cycle.join("', '");
        for group in &groups.cycle {
            remap_group_issues.push((
                group.clone(),
                Severity::Error,
                format!(
                    "remap group '{}' is in an ordering cycle with '{}'; \
                 the groups fall back to source order",
                    group, names,
                ),
            ));
        }
    }
    for group in &groups.duplicate_decls {
        remap_group_issues.push((
            group.clone(),
            Severity::Error,
            format!("remap group '{}' is declared more than once", group,),
        ));
    }
    // Over `order`, not over `info`: a HashMap would make the report's wording
    // stable but its order not.
    for group in &groups.order {
        let info = &groups.info[group];
        if info.declared && !info.has_rules {
            remap_group_issues.push((
                group.clone(),
                Severity::Warning,
                format!("remap group '{}' is declared but has no rules", group,),
            ));
        }
    }

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                DocumentItem::Glyph {
                    name: GlyphName(n),
                    body,
                } => {
                    for bad in find_invalid_inline_ranges(n) {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("invalid inline range '{}' (end < start or too large)", bad,),
                        ));
                    }
                    // Duplicate detection needs the *defining* line of each
                    // expanded name, which the expansion does not retain, so
                    // it expands names (not bodies) once more here.
                    let name_str = substitute_name_parts(n, name_parts);
                    let expanded: Vec<String> = if is_name_pattern(&name_str) {
                        // A pattern that fails to expand is already reported
                        // by the resolution pass.
                        NamePattern::parse(&name_str)
                            .map(|e| e.into_vec())
                            .unwrap_or_default()
                    } else {
                        vec![name_str]
                    };
                    for en in expanded {
                        if let Some((prev_file, prev_line)) = glyph_defs.get(en.as_str()) {
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Warning,
                                format!(
                                    "duplicate glyph '{}' (first defined at {}:{})",
                                    en,
                                    short_path(prev_file),
                                    prev_line,
                                ),
                            ));
                        } else {
                            let (_, file_line) = doc.item_lines(item_idx);
                            glyph_defs.insert(en, (doc.path.clone(), file_line));
                        }
                    }

                    if body.pixels.is_none()
                        && body.refs.is_empty()
                        && !body.sticky
                        && body.advance.is_none()
                        && body.left.is_none()
                        && body.points.is_empty()
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Warning,
                            format!("glyph '{}' has no content", n,),
                        ));
                    }
                }
                DocumentItem::RemapGroup { name, .. } => {
                    // Every group-level problem is reported here, on the line
                    // that declares the group — the constraint is written here
                    // even where its effect is felt somewhere else entirely.
                    for (group, severity, message) in &remap_group_issues {
                        if group == name {
                            issues.push(issue_at(doc, item_idx, severity.clone(), message.clone()));
                        }
                    }
                }
                DocumentItem::Remap {
                    feature,
                    source,
                    target,
                    ..
                } => {
                    // OpenType has a lookup type for one-to-one, one-to-many
                    // (including one-to-nothing) and many-to-one, and nothing
                    // for the rest. The builder used to emit whatever was
                    // closest and lose the difference in silence.
                    if crate::render::ttf_builder::remap_rule_kind(source.len(), target.len())
                        .is_none()
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "remap of {} glyph(s) to {} glyph(s) has no OpenType lookup type; \
                             a source of more than one glyph needs exactly one target",
                                source.len(),
                                target.len(),
                            ),
                        ));
                    }

                    // The reverse lookup substitutes one glyph for one glyph
                    // and has no ligature or multiple form at all, so a rule of
                    // any other shape cannot be built here — and rebuilding the
                    // group forward to accommodate it would silently take away
                    // the very thing `reversed` was asked for.
                    if groups.info.get(feature).is_some_and(|i| i.reversed)
                        && (source.len() != 1 || target.len() != 1)
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "remap group '{}' is reversed, so each of its rules must \
                             substitute one glyph for one glyph; this one has {} source \
                             glyph(s) and {} target glyph(s)",
                                feature,
                                source.len(),
                                target.len(),
                            ),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // `meta` validation. Two passes over different things: each line on its own
    // (does it parse, is its key already taken), then the effective numbers.
    // For the latter, the distinction between "not declared" and "declared as
    // the default" is what decides whether to complain at all.
    {
        // Slot -> scope -> where it was declared. A slot set twice in one
        // scope is an outright duplicate; a slot set both bare and for a face
        // gives *that face* two values, which is the same conflict a face
        // including two slices that map one character has. There is no
        // precedence rule in either place, by design.
        let mut declared_meta: BTreeMap<String, BTreeMap<Option<String>, ItemRef>> =
            BTreeMap::new();
        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let DocumentItem::Meta(text) = item else {
                    continue;
                };
                let here = ItemRef::new(doc_idx, item_idx);
                let (scope, entry) = match crate::meta::parse_meta_entry(text) {
                    Ok(parsed) => parsed,
                    Err(message) => {
                        issues.push(docset.to_issue(&Diagnostic::error(here, message)));
                        continue;
                    }
                };
                if let Some(face) = &scope
                    && !faces.faces.iter().any(|f| &f.id == face)
                {
                    issues.push(docset.to_issue(&Diagnostic::error(
                        here,
                        format!("`meta` is scoped to undeclared face `{face}`"),
                    )));
                    continue;
                }
                let by_scope = declared_meta.entry(entry.slot()).or_default();
                if let Some(&first) = by_scope.get(&scope) {
                    let (path, _, file_line) = docset.location(first);
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    issues.push(docset.to_issue(&Diagnostic::error(
                        here,
                        format!(
                            "{} is set more than once (also at {name}:{file_line})",
                            entry.describe_slot(),
                        ),
                    )));
                } else {
                    by_scope.insert(scope, here);
                }
            }
        }
        for (slot, by_scope) in &declared_meta {
            if by_scope.len() < 2 {
                continue;
            }
            for face in &faces.faces {
                let reaching: Vec<(&Option<String>, &ItemRef)> = by_scope
                    .iter()
                    .filter(|(scope, _)| match scope {
                        None => true,
                        Some(f) => *f == face.id,
                    })
                    .collect();
                if reaching.len() < 2 {
                    continue;
                }
                let (_, first) = reaching[0];
                let (path, _, file_line) = docset.location(*first);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                for (_, here) in &reaching[1..] {
                    issues.push(docset.to_issue(&Diagnostic::error(
                        **here,
                        format!(
                            "`meta {slot}` is set both for every face and for face `{}`, \
                             so face `{}` has two values (the other at {name}:{file_line})",
                            face.label(),
                            face.label(),
                        ),
                    )));
                }
            }
        }

        // Two faces the OS cannot tell apart are two faces the user cannot pick
        // between, and a duplicate PostScript name additionally breaks PDF
        // embedding. Checked across faces because no single face can see it.
        if faces.faces.len() > 1 {
            let mut seen_full: HashMap<(String, String), &str> = HashMap::new();
            let mut seen_ps: HashMap<String, &str> = HashMap::new();
            for face in &faces.faces {
                let id = if face.id.is_empty() {
                    None
                } else {
                    Some(face.id.as_str())
                };
                let m = crate::meta::FontMeta::for_face(docs, id);
                let key = (m.family().to_string(), m.subfamily().to_string());
                if let Some(prev) = seen_full.get(&key) {
                    issues.push(docset.to_issue(&Diagnostic::error(
                        face.origin,
                        format!(
                            "faces `{prev}` and `{}` both name themselves `{} {}`; \
                             the OS files fonts by family and subfamily, so one would hide \
                             the other",
                            face.label(),
                            key.0,
                            key.1,
                        ),
                    )));
                } else {
                    seen_full.insert(key, face.label());
                }
                let ps = m.postscript_name();
                if let Some(prev) = seen_ps.get(&ps) {
                    issues.push(docset.to_issue(&Diagnostic::error(
                        face.origin,
                        format!(
                            "faces `{prev}` and `{}` share the PostScript name `{ps}`",
                            face.label(),
                        ),
                    )));
                } else {
                    seen_ps.insert(ps, face.label());
                }
            }
        }

        let meta = &resolution.meta;
        let origin = meta.origin;
        if let (Some(h), Some(a), Some(d)) = (
            meta.metrics.height,
            meta.metrics.ascent,
            meta.metrics.descent,
        ) && a as u32 + d as u32 != h as u32
        {
            issues.push(docset.to_issue(&Diagnostic::new(
                Severity::Warning,
                origin,
                format!("meta ascent ({a}) + descent ({d}) != height ({h})"),
            )));
        }
        if meta.metrics.height == Some(0) {
            issues.push(docset.to_issue(&Diagnostic::error(origin, "meta height is 0")));
        }
    }

    /// Where one codepoint was mapped: the file, the `DocLine` index and the
    /// 1-based file line, as [`Issue`] wants them.
    struct MapSite {
        file: PathBuf,
        line: usize,
        file_line: usize,
    }

    // Every codepoint, by the slice that maps it. Two entries in one slice are
    // the duplicate this has always warned about; two entries in *different*
    // slices are only a problem for a face that includes both, which is the
    // conflict the face split exists to make explicit.
    let mut mapped_codepoints: HashMap<u32, BTreeMap<Option<String>, MapSite>> = HashMap::new();
    let mut mapped_glyphs: HashSet<String> = HashSet::new();

    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                // Unresolvable refs, map targets and remap operands are all
                // reported by the resolution pass above.
                DocumentItem::Map {
                    slices,
                    char_repr,
                    glyph,
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
                        let subst_glyph =
                            substitute_name_parts(glyph, scoped_parts.for_slice(slice.as_deref()));
                        let expanded_pairs =
                            crate::render::ttf_builder::expand_map_pairs(char_repr, &subst_glyph);
                        for (cp, target) in &expanded_pairs {
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
                DocumentItem::Directive(text) => {
                    // `font-meta` became `meta`, one key per line. This is an
                    // error, not the usual unrecognized-directive warning: the
                    // font builds through warnings, and it would build with
                    // default metrics while the file plainly states others.
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
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Warning,
                            format!("unrecognized directive '{}'", text.trim(),),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // Detect unused glyphs: glyphs not reachable from any map/remap root.
    // Works at glyph-item granularity to avoid expensive repeated pattern expansion.
    {
        // Assign each glyph item an index; track which items are reachable.
        // name_to_item: expanded glyph name -> item index
        let mut name_to_item: HashMap<String, usize> = HashMap::new();
        // item_refs[i]: ref target names (expanded) for item i
        let mut item_refs: Vec<Vec<String>> = Vec::new();
        // item_location[i]: (doc_idx, item_idx, raw_name, is_alias) for reporting
        let mut item_location: Vec<(usize, usize, &str, bool)> = Vec::new();

        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                // An alias is a node of this graph like any glyph: it is
                // reachable from what names it and it keeps its target
                // reachable in turn, so `map x = A` where `glyph A = B` leaves
                // neither the alias nor `B` looking unused.
                let (name, refs, is_alias) = match item {
                    DocumentItem::Glyph {
                        name: GlyphName(n),
                        body,
                    } => {
                        let mut refs = Vec::new();
                        for gref in &body.refs {
                            refs.extend(expand_name_element(&gref.name, name_parts));
                        }
                        (n, refs, false)
                    }
                    DocumentItem::GlyphAlias {
                        name: GlyphName(n),
                        target,
                        ..
                    } => (n, expand_name_element(target, name_parts), true),
                    _ => continue,
                };

                let idx = item_refs.len();
                item_location.push((doc_idx, item_idx, name, is_alias));
                for en in expand_name_element(name, name_parts) {
                    name_to_item.entry(en).or_insert(idx);
                }
                item_refs.push(refs);
            }
        }

        // Collect root names from map targets and remap references.
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

        let mut root_names: HashSet<String> = mapped_glyphs;
        // .notdef is always required in TrueType fonts.
        root_names.insert(".notdef".to_string());

        for doc in docs {
            for item in &doc.items {
                match item {
                    DocumentItem::Remap { .. } => {
                        for token in item.remap_operands() {
                            root_names.extend(expand_name_element(token, name_parts));
                        }
                    }
                    DocumentItem::Glyph {
                        name: GlyphName(n),
                        body,
                    } => {
                        if body.sticky || body.mark {
                            root_names.extend(expand_name_element(n, name_parts));
                        }
                    }
                    DocumentItem::Directive(text) => {
                        if let Directive::AssumeUnused(rest) = classify_directive(text) {
                            for token in rest.split_whitespace() {
                                root_names.extend(expand_name_element(token, name_parts));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Build alternative lookup: base name -> list of "base:variant" names.
        let mut alt_names: HashMap<&str, Vec<&str>> = HashMap::new();
        for name in all_glyph_names.iter() {
            if let Some(colon_pos) = name.find(':') {
                let base = &name[..colon_pos];
                alt_names.entry(base).or_default().push(name.as_str());
            }
        }

        // BFS over items.
        let mut reachable_items: Vec<bool> = vec![false; item_refs.len()];
        let mut queue: Vec<usize> = Vec::new();

        // Seed with root names.
        for name in &root_names {
            if let Some(&idx) = name_to_item.get(name.as_str())
                && !reachable_items[idx]
            {
                reachable_items[idx] = true;
                queue.push(idx);
            }
            // Alternatives of root names are also roots.
            if let Some(alts) = alt_names.get(name.as_str()) {
                for &alt in alts {
                    if let Some(&idx) = name_to_item.get(alt)
                        && !reachable_items[idx]
                    {
                        reachable_items[idx] = true;
                        queue.push(idx);
                    }
                }
            }
        }

        while let Some(item_idx) = queue.pop() {
            for ref_name in &item_refs[item_idx] {
                if let Some(&target_item) = name_to_item.get(ref_name.as_str())
                    && !reachable_items[target_item]
                {
                    reachable_items[target_item] = true;
                    queue.push(target_item);
                }
                // Alternatives of ref targets are also reachable.
                if let Some(alts) = alt_names.get(ref_name.as_str()) {
                    for &alt in alts {
                        if let Some(&alt_item) = name_to_item.get(alt)
                            && !reachable_items[alt_item]
                        {
                            reachable_items[alt_item] = true;
                            queue.push(alt_item);
                        }
                    }
                }
            }
        }

        // Report unreachable items.
        for (idx, &reached) in reachable_items.iter().enumerate() {
            if !reached {
                let (doc_idx, doc_item_idx, name, is_alias) = item_location[idx];
                let doc = docs[doc_idx];
                let what = if is_alias { "glyph alias" } else { "glyph" };
                issues.push(issue_at(
                    doc,
                    doc_item_idx,
                    Severity::Warning,
                    format!("{what} '{}' is unused", name,),
                ));
            }
        }
    }

    // Detect alternative glyphs with ambiguous anchor matches.
    // For base "foo", if "foo" and "foo:bar" both have a `-name` anchor with
    // the same dimensions, warn that they are ambiguous (the first alphabetically wins).
    {
        let mut bases_to_alts: HashMap<String, Vec<(String, PathBuf, usize, usize)>> =
            HashMap::new();
        for doc in docs {
            for (item_idx, item) in doc.items.iter().enumerate() {
                if let DocumentItem::Glyph {
                    name: GlyphName(n),
                    body,
                } = item
                    && body.points.iter().any(|p| p.position.starts_with('-'))
                {
                    let resolved_name = substitute_name_parts(n, name_parts);
                    let (line, file_line) = doc.item_lines(item_idx);
                    // Find all base prefixes (foo:bar:quux is alt for "foo" and "foo:bar")
                    for prefix in crate::ref_composite::alternative_prefixes(&resolved_name) {
                        bases_to_alts.entry(prefix.to_string()).or_default().push((
                            resolved_name.clone(),
                            doc.path.clone(),
                            line,
                            file_line,
                        ));
                    }
                    // Also register as the base itself
                    bases_to_alts
                        .entry(resolved_name.clone())
                        .or_default()
                        .push((resolved_name.clone(), doc.path.clone(), line, file_line));
                }
            }
        }

        // For each base, find point definitions and check for dimension conflicts.
        let mut glyph_points_map: HashMap<String, Vec<(String, u16, u16)>> = HashMap::new();
        for doc in docs {
            for item in &doc.items {
                if let DocumentItem::Glyph {
                    name: GlyphName(n),
                    body,
                } = item
                {
                    let resolved_name = substitute_name_parts(n, name_parts);
                    for pt in &body.points {
                        if pt.position.starts_with('-') {
                            glyph_points_map
                                .entry(resolved_name.clone())
                                .or_default()
                                .push((pt.position.clone(), pt.width(), pt.height()));
                        }
                    }
                }
            }
        }

        for (base, alts) in &bases_to_alts {
            if alts.len() < 2 {
                continue;
            }
            // Group by (position_name, width, height) and find duplicates.
            let mut seen: HashMap<(String, u16, u16), Vec<&str>> = HashMap::new();
            for (alt_name, _, _, _) in alts {
                if let Some(pts) = glyph_points_map.get(alt_name) {
                    for (pos, w, h) in pts {
                        seen.entry((pos.clone(), *w, *h))
                            .or_default()
                            .push(alt_name);
                    }
                }
            }
            for ((pos, _w, _h), names) in &seen {
                if names.len() > 1 {
                    // Warn on all but the first (alphabetically).
                    let mut sorted_names: Vec<&str> = names.to_vec();
                    sorted_names.sort();
                    for &dup in &sorted_names[1..] {
                        if let Some((_, file, line, file_line)) =
                            alts.iter().find(|(n, _, _, _)| n == dup)
                        {
                            issues.push(Issue {
                                severity: Severity::Warning,
                                message: format!(
                                    "alternative '{}' has same anchor dimensions as '{}' for '{}' (base '{}')",
                                    dup, sorted_names[0], pos, base,
                                ),
                                file: file.clone(),
                                line: *line,
                                file_line: *file_line,
                            });
                        }
                    }
                }
            }
        }
    }

    // Anchor derivation problems: a composite that would expose the same
    // anchor name from more than one source, and a `-` anchor with more than
    // one `+` candidate to attach to. This runs an anchors-only pass through
    // the same shared driver and the same derivation the font build uses
    // (`glyph_cache`/`derive_ref_offsets_with`), so what is reported here is
    // exactly what resolution dropped.
    {
        struct AnchorsOnly {
            anchors: Vec<crate::document::GlyphPoint>,
            w: u16,
            h: u16,
        }
        impl AnchorsOnly {
            fn new() -> Self {
                Self {
                    anchors: Vec::new(),
                    w: 0,
                    h: 0,
                }
            }
        }
        impl crate::render::glyph_cache::CachedGlyphEntry for AnchorsOnly {
            fn anchors(&self) -> &[crate::document::GlyphPoint] {
                &self.anchors
            }
            fn dims_mut(&mut self) -> (&mut u16, &mut u16) {
                (&mut self.w, &mut self.h)
            }
            fn set_resolution(&mut self, anchors: Vec<crate::document::GlyphPoint>, _scale: u8) {
                self.anchors = anchors;
            }
        }

        let mut declared_anchors: HashMap<&str, &[crate::document::GlyphPoint]> = HashMap::new();
        let mut origin_of: HashMap<&str, Option<crate::resolve::ItemRef>> = HashMap::new();
        for e in &expansion.items {
            if let DocumentItem::Glyph {
                name: GlyphName(n),
                body,
            } = &e.item
            {
                declared_anchors.entry(n).or_insert(&body.points);
                origin_of.entry(n).or_insert(e.origin);
            }
        }

        let (mut cache, pending) = crate::render::glyph_cache::seed_cache(
            expansion.items(),
            |_| AnchorsOnly::new(),
            AnchorsOnly::new,
        );
        let mut derive_issues: Vec<(String, crate::ref_composite::DeriveIssue)> = Vec::new();
        crate::render::glyph_cache::resolve_pending(
            &mut cache,
            pending,
            |name| declared_anchors.get(name).map(|pts| pts.to_vec()),
            |_, _, _| AnchorsOnly::new(),
            |name, issue| derive_issues.push((name.to_string(), issue)),
        );
        for (name, issue) in derive_issues {
            let severity = if issue.is_error() {
                Severity::Error
            } else {
                Severity::Warning
            };
            issues.push(docset.to_issue(&Diagnostic::new(
                severity,
                origin_of.get(name.as_str()).copied().flatten(),
                issue.message(&name),
            )));
        }
    }

    // A `color` alias or a `ref ... fill` naming a color nothing declares
    // falls back to `fg` in the build without a word. Aliases resolve in
    // document order, earlier declarations only (see
    // `render::ttf_builder::color::collect_color_aliases`); asking that same
    // collection which names made it into the map mirrors the build exactly.
    {
        let color_aliases = crate::render::ttf_builder::collect_color_aliases(docs);
        for doc in docs {
            for (item_idx, item) in doc.items.iter().enumerate() {
                match item {
                    DocumentItem::Color { name, value, .. } => {
                        if !color_aliases.contains_key(name) {
                            let why = if value.starts_with('#') {
                                format!("invalid color value `{value}`")
                            } else if color_aliases.contains_key(value) {
                                format!(
                                    "`{value}` is declared later, and color aliases resolve \
                                     in document order"
                                )
                            } else {
                                format!("undeclared color `{value}`")
                            };
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Warning,
                                format!("color `{name}` never resolves: {why}"),
                            ));
                        }
                    }
                    DocumentItem::Glyph { body, .. } => {
                        for gref in &body.refs {
                            let Some(fill) = &gref.fill else { continue };
                            let c = &fill.color;
                            if c == "fg" {
                                continue;
                            }
                            if c.starts_with('#') {
                                if crate::render::ttf_builder::parse_hex_color(c).is_none() {
                                    issues.push(issue_at(
                                        doc,
                                        item_idx,
                                        Severity::Warning,
                                        format!("invalid fill color `{c}`"),
                                    ));
                                }
                            } else if !color_aliases.contains_key(c.as_str()) {
                                issues.push(issue_at(
                                    doc,
                                    item_idx,
                                    Severity::Warning,
                                    format!(
                                        "fill names undeclared color `{c}`; it renders as `fg`"
                                    ),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    issues
}

impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let rank = |s: &Severity| match s {
            Severity::Error => 0,
            Severity::Warning => 1,
        };
        rank(self).cmp(&rank(other))
    }
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io;

    #[test]
    fn unresolved_ref_reported() {
        let input = "glyph foo\nref nonexistent 0 0\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("unresolved ref")),
            "expected unresolved ref error, got: {issues:?}",
        );
    }

    #[test]
    fn duplicate_inherited_anchors_reported() {
        let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0 inherit
ref half 2 0 inherit
map D = digraph
map h = half
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("digraph")
                && i.message.contains("'+above'")),
            "expected duplicate exposed anchor error, got: {issues:?}",
        );
    }

    #[test]
    fn ambiguous_attachment_reported() {
        let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph mark 2 1 mark
@@@@
anchor -above 0 0
glyph combo
ref half 0 0
ref half 2 0
ref mark
map D = combo
map h = half
map m = mark
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("combo")
                && i.message.contains("'mark'")
                && i.message.contains("'-above'")),
            "expected ambiguous attachment error, got: {issues:?}",
        );
    }

    /// A `-` anchor that name-matches a published `+` but size-mismatches it
    /// is a near-miss (usually the wrong `:narrow`/`:wide` variant), reported
    /// as a warning rather than silently not attaching.
    #[test]
    fn size_mismatched_attachment_reported() {
        let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1..2 0
glyph mark 2 1 mark
@@@@
anchor -above 0 0
glyph combo
ref base
ref mark 1 2
map D = combo
map h = base
map m = mark
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("combo")
                && i.message.contains("'mark'")
                && i.message.contains("'-above'")),
            "expected size-mismatch warning, got: {issues:?}",
        );
    }

    /// The validation pass must resolve an alternative *before* any composite
    /// that needs it for size-driven substitution — same guard as the
    /// editor's `resolve_expansion` — or it reports a mismatch the real
    /// resolution does not have.
    #[test]
    fn alternative_pending_in_same_round_still_substitutes() {
        let input = "\
glyph circle 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph circle:alt
ref circle
anchor +center 2 1

glyph j-inner 2 2
@@@@
@@@@
anchor -center 1 0

glyph j-circled
ref circle
ref j-inner
map j = j-circled
map c = circle
map i = j-inner
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("only by name")),
            "circle:alt must be substituted, got: {issues:?}",
        );
    }

    /// A digraph without `inherit` exposes nothing — that is the designed
    /// fallback, not a problem to report.
    #[test]
    fn non_inherited_duplicates_are_quiet() {
        let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0
ref half 2 0
map D = digraph
map h = half
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error),
            "expected no errors, got: {issues:?}",
        );
    }

    #[test]
    fn duplicate_glyph_reported() {
        let input = "glyph foo 2 1\n..@@\nglyph foo 2 1\n@@..\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.message.contains("duplicate glyph")),
            "expected duplicate glyph warning, got: {issues:?}",
        );
    }

    #[test]
    fn undefined_map_target_reported() {
        let input = "map A = nonexistent\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("nonexistent")),
            "expected undefined map target error, got: {issues:?}",
        );
    }

    #[test]
    fn valid_document_has_no_issues() {
        let input = "\
glyph foo 2 1
..@@
map A = foo
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(issues.is_empty(), "expected no issues, got: {issues:?}",);
    }

    // `testdata/` declares a single consistent `meta` because it has to
    // stay a coherent project, so the broken variants are covered here.

    #[test]
    fn a_map_to_a_contentless_glyph_is_an_error() {
        // Neither a pixel grid nor a ref means the glyph never enters the
        // resolution cache, so it silently vanishes from the cmap. `advance`
        // does not make it buildable, but it does suppress the "has no
        // content" warning, so this used to pass without a single word.
        let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
map B = blank
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("'blank'")
                && i.message.contains("not built")),
            "mapping a contentless glyph must be an error, got: {issues:?}",
        );
    }

    #[test]
    fn a_ref_and_a_remap_to_a_contentless_glyph_are_errors() {
        let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
glyph host
ref blank
map A = vis
remap liga : vis -> blank
feature liga for DFLT : liga
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error && i.message.contains("'blank'"))
            .collect();
        assert!(
            errors.len() >= 2,
            "both the ref and the remap must be reported, got: {issues:?}",
        );
    }

    /// A glyph that is contentless but never used stays a warning — it builds
    /// nothing, but it also breaks nothing.
    #[test]
    fn an_unused_contentless_glyph_is_not_an_error() {
        let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
assume unused blank
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error),
            "an unused contentless glyph must not be an error, got: {issues:?}",
        );
    }

    // ------------------------------------------------------------------
    // Glyph aliases (`glyph NAME = TARGET`); see `crate::alias`.
    // ------------------------------------------------------------------

    fn issues_for(input: &str) -> Vec<Issue> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
    }

    fn has(issues: &[Issue], severity: Severity, needle: &str) -> bool {
        issues
            .iter()
            .any(|i| i.severity == severity && i.message.contains(needle))
    }

    /// A `fill` naming a color that no `color` line declares silently fell
    /// back to `fg` in the build; it has to be reported here instead.
    #[test]
    fn a_fill_naming_an_undeclared_color_is_a_warning() {
        let issues = issues_for("glyph a 1 1\n@@\n\nglyph b\nref a fill missing\n\nmap A = b\n");
        assert!(
            has(&issues, Severity::Warning, "undeclared color `missing`"),
            "{issues:?}"
        );
    }

    /// `color` aliases resolve in document order (see
    /// `render::ttf_builder::color::collect_color_aliases`), so a value naming
    /// a color declared later never resolves — silently, before this check.
    #[test]
    fn a_color_alias_used_before_its_declaration_is_a_warning() {
        let issues = issues_for("color x = y\ncolor y = #ff0000\n");
        assert!(has(&issues, Severity::Warning, "color `x`"), "{issues:?}");
    }

    #[test]
    fn declared_color_uses_are_quiet() {
        let issues = issues_for(
            "color red = #ff0000\ncolor also-red = red\n\nglyph a 1 1\n@@\n\n\
             glyph b\nref a fill also-red\n\nmap A = b\n",
        );
        assert!(
            !issues.iter().any(|i| i.message.contains("color")),
            "{issues:?}"
        );
    }

    #[test]
    fn an_alias_to_an_undefined_glyph_is_an_error() {
        let issues = issues_for(
            "\
glyph pix 1 1
@@
glyph a = nope
map A = pix
map B = a
",
        );
        assert!(
            has(&issues, Severity::Error, "names undefined glyph `nope`"),
            "{issues:?}",
        );
    }

    /// An alias is a second name for a glyph, so a name that is both is two
    /// answers to one question — and the expansion would silently keep the
    /// glyph and drop the alias.
    #[test]
    fn a_name_that_is_both_a_glyph_and_an_alias_is_an_error() {
        let issues = issues_for(
            "\
glyph pix 1 1
@@
glyph a 1 1
@@
glyph a = pix
map A = a
",
        );
        assert!(
            has(&issues, Severity::Error, "both a glyph and an alias"),
            "{issues:?}",
        );
    }

    #[test]
    fn an_alias_cycle_is_an_error() {
        let issues = issues_for(
            "\
glyph pix 1 1
@@
glyph a = b
glyph b = a
map A = pix
",
        );
        assert!(has(&issues, Severity::Error, "is in a cycle"), "{issues:?}");
    }

    #[test]
    fn a_duplicate_alias_is_an_error() {
        let issues = issues_for(
            "\
glyph pix 1 1
@@
glyph other 1 1
..
glyph a = pix
glyph a = other
map A = a
",
        );
        assert!(
            has(&issues, Severity::Error, "declared more than once"),
            "{issues:?}",
        );
    }

    /// An alias nothing names is dead source, reported like an unused glyph —
    /// but named as what it is, since the fix is to delete a line rather than
    /// to find a home for a drawing.
    #[test]
    fn an_unused_alias_is_a_warning() {
        let issues = issues_for(
            "\
glyph pix 1 1
@@
glyph a = pix
map A = pix
",
        );
        assert!(
            has(&issues, Severity::Warning, "glyph alias 'a' is unused"),
            "{issues:?}",
        );
    }

    /// The alias is a node of the reachability walk: naming it must keep both
    /// it and its target alive.
    #[test]
    fn a_used_alias_keeps_its_target_used() {
        let issues = issues_for(
            "\
glyph pix 1 1
@@
glyph a = pix
map A = a
",
        );
        assert!(
            !issues.iter().any(|i| i.message.contains("unused")),
            "neither the alias nor its target is unused, got: {issues:?}",
        );
    }

    /// Pattern glyphs already refuse to be empty, whatever the reason — a
    /// pixel grid cannot be shared across the expansions, so only `ref` lines
    /// can fill them.
    #[test]
    fn an_empty_pattern_glyph_is_an_error() {
        for body in ["", " advance 0"] {
            let input = format!(
                "\
name-parts $ab = a b

glyph pix 1 1
@@
glyph pat-($ab){body}
map A|B = pat-($ab)
"
            );
            let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
            let issues = collect_issues(&[&doc]);
            assert!(
                issues
                    .iter()
                    .any(|i| i.severity == Severity::Error
                        && i.message.contains("defines no glyphs")),
                "an empty pattern glyph must be an error (body {body:?}), got: {issues:?}",
            );
        }
    }

    /// A `name-parts` value is a pattern, so the declaration itself can be
    /// over the expansion limit — before any glyph line refers to it.
    #[test]
    fn an_oversized_name_parts_binding_is_an_error() {
        let input = format!(
            "name-parts $many = x($1..{})\n",
            crate::pattern::MAX_EXPANSION + 1
        );
        let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("name part `$many`")),
            "an oversized binding must be an error, got: {issues:?}",
        );
    }

    #[test]
    fn many_to_many_remap_is_an_error() {
        // Neither a ligature nor a multiple substitution can express this, and
        // guessing one of them silently loses half the rule.
        let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
glyph d 1 1
@@
map A = a
map B = b
remap liga : a b -> c d
feature liga for DFLT : liga
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("no OpenType lookup type")),
            "a 2-to-2 remap must be an error, got: {issues:?}",
        );
    }

    #[test]
    fn many_to_nothing_remap_is_an_error() {
        let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
map A = a
map B = b
remap liga : a b ->
feature liga for DFLT : liga
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("no OpenType lookup type")),
            "deleting a multi-glyph sequence must be an error, got: {issues:?}",
        );
    }

    #[test]
    fn expressible_remap_shapes_are_quiet() {
        // one-to-one, one-to-many, one-to-nothing and many-to-one all have a
        // lookup type, so none of them may be reported.
        let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
map A = a
map B = b
map C = c
remap g1 : a -> b
remap g2 : a -> b c
remap g3 : a ->
remap g4 : a b -> c
feature liga for DFLT : g1
feature liga for DFLT : g2
feature liga for DFLT : g3
feature liga for DFLT : g4
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("no OpenType lookup type")),
            "expressible remaps must be quiet, got: {issues:?}",
        );
    }

    #[test]
    fn remap_pattern_operand_expansions_are_checked() {
        // Remap operands keep their patterns until the GSUB builder expands
        // them, and that builder drops rules whose glyphs have no id without
        // a word. Validation therefore has to expand them the same way.
        let input = "\
name-parts $ab = a b

glyph ok 2 1
@@..
map A = ok

remap liga : ok -> missing-($ab)
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.message.contains("missing-a")),
            "expected the expanded remap target to be reported, got: {issues:?}",
        );
        assert!(
            issues.iter().any(|i| i.message.contains("missing-b")),
            "every expansion should be reported, got: {issues:?}",
        );
    }

    #[test]
    fn remap_pattern_operand_that_resolves_is_quiet() {
        let input = "\
name-parts $ab = a b

glyph ok 2 1
@@..
glyph present-a 2 1
@@..
glyph present-b 2 1
..@@
map A = ok

remap liga : ok -> present-($ab)
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("remap")),
            "a remap whose expansions all exist must be quiet, got: {issues:?}",
        );
    }

    #[test]
    fn meta_ascent_plus_descent_must_equal_height() {
        let input = "meta height 16\nmeta ascent 12\nmeta descent 3\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Warning && i.message.contains("!= height")),
            "expected meta metric mismatch warning, got: {issues:?}",
        );
    }

    #[test]
    fn meta_zero_height_reported() {
        let input = "meta height 0\nmeta ascent 0\nmeta descent 0\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("meta height is 0")),
            "expected zero-height error, got: {issues:?}",
        );
    }

    /// An unknown key is the whole reason `meta` exists as a checked directive:
    /// the value it carries is invisible in the built font, so a typo that is
    /// merely ignored is a typo that ships.
    #[test]
    fn meta_unknown_key_is_error() {
        let input = "meta famliy 16\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("famliy")),
            "expected unknown-key error, got: {issues:?}",
        );
    }

    /// Every kind of conflict is an error, and two `meta` lines setting the
    /// same key are a conflict even when they agree.
    #[test]
    fn meta_duplicate_key_is_error() {
        let input = "meta height 16\nmeta height 16\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("height")
                && i.message.contains("more than once")),
            "expected duplicate-key error, got: {issues:?}",
        );
    }

    #[test]
    fn meta_wrong_arity_is_error() {
        for input in ["meta height\n", "meta height 16 12\n"] {
            let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
            let issues = collect_issues(&[&doc]);
            assert!(
                issues.iter().any(|i| i.severity == Severity::Error),
                "expected an arity error for {input:?}, got: {issues:?}",
            );
        }
    }

    #[test]
    fn meta_non_numeric_metric_is_error() {
        let input = "meta height sixteen\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error),
            "expected a non-numeric error, got: {issues:?}",
        );
    }

    /// `font-meta` became `meta`. A leftover line must not fall through to the
    /// generic "unrecognized directive" *warning*: the font would then build
    /// with default metrics, silently ignoring the metrics the file states.
    #[test]
    fn legacy_font_meta_is_error() {
        let input = "font-meta height 16 ascent 12 descent 4\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("font-meta")
                && i.message.contains("meta")),
            "expected a migration error, got: {issues:?}",
        );
    }

    #[test]
    fn duplicate_alternative_anchor_warns() {
        let input = "\
glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:a 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:b 2 2
@@@@
@@@@
anchor -join 0 0
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Warning
                    && i.message.contains("same anchor dimensions")),
            "expected duplicate alternative anchor warning, got: {issues:?}",
        );
    }

    #[test]
    fn unused_glyph_reported() {
        let input = "\
glyph used 2 1
..@@
map A = used

glyph orphan 2 1
@@..
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.message.contains("glyph 'orphan' is unused")),
            "expected unused glyph warning, got: {issues:?}",
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("glyph 'used' is unused")),
            "mapped glyph should not be reported as unused",
        );
    }

    #[test]
    fn transitively_used_glyph_not_reported() {
        let input = "\
glyph base 2 1
..@@

glyph composite 2 1
@@..
ref base 0 0

map A = composite
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("is unused")),
            "transitively used glyph should not be unused: {issues:?}",
        );
    }

    #[test]
    fn mutually_referencing_cluster_reported() {
        let input = "\
glyph a 2 1
..@@
ref b 0 0

glyph b 2 1
@@..
ref a 0 0
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("glyph 'a' is unused")),
            "mutual ref cluster should be unused: {issues:?}",
        );
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("glyph 'b' is unused")),
            "mutual ref cluster should be unused: {issues:?}",
        );
    }

    #[test]
    fn remap_target_counts_as_used() {
        let input = "\
glyph base 2 1
..@@
map A = base

glyph alt 2 1
@@..

remap liga : base -> alt
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("glyph 'alt' is unused")),
            "remap target should count as used: {issues:?}",
        );
    }

    #[test]
    fn alternative_glyph_used_when_base_used() {
        let input = "\
glyph stem 2 1
..@@
map A = stem

glyph stem:wide 2 1
@@..
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("glyph 'stem:wide' is unused")),
            "alternative of used base should not be unused: {issues:?}",
        );
    }

    #[test]
    fn sticky_glyph_not_reported_unused() {
        let input = "glyph keep sticky advance 0\n";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("is unused")),
            "sticky glyph should not be unused: {issues:?}",
        );
    }

    #[test]
    fn ref_to_bodiless_sticky_glyph_not_reported_unbuilt() {
        // A dimension-less `glyph NAME sticky` is a placeholder that *is*
        // built (an empty anchor-carrying entry, see `glyph_cache::seed_cache`)
        // and is exempt from the "has no content" warning above; the
        // expansion's "is not built" error must exempt it the same way.
        let input = "\
glyph keep sticky
anchor +join 0 0

glyph user 2 1
@@..
ref keep
map A = user
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("not built")),
            "sticky placeholder is built; a ref to it is fine: {issues:?}",
        );
    }

    #[test]
    fn assert_same_distinct_not_unrecognized() {
        let input = "\
glyph a 2 1
..@@
glyph b 2 1
@@..
map A = a
map B = b

assert same a b
assert distinct a b
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("unrecognized directive")),
            "assert same/distinct should not be flagged as unrecognized: {issues:?}",
        );
    }

    #[test]
    fn map_decomposed_without_decomposition_reported() {
        // 'A' is already in NFD, so `map A` cannot synthesize anything.
        let input = "\
glyph a 2 1
..@@
map U+0041 = a
map generate A
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("no canonical decomposition")),
            "expected no-decomposition error, got: {issues:?}",
        );
    }

    #[test]
    fn map_decomposed_with_unmapped_component_reported() {
        // 'Ä' decomposes to U+0041 U+0308; U+0308 is not mapped.
        let input = "\
glyph a 2 1
..@@
map A = a
map generate Ä
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("unmapped codepoint")
                && i.message.contains("U+0308")),
            "expected unmapped component error, got: {issues:?}",
        );
    }

    #[test]
    fn map_decomposed_fully_mapped_accepted() {
        let input = "\
glyph a 2 1
..@@
glyph dieresis 2 1
@@..
map A = a
map U+0308 = dieresis
map generate Ä
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("decomposition")
                    || i.message.contains("unmapped codepoint")),
            "fully mapped decomposition should be accepted, got: {issues:?}",
        );
    }

    #[test]
    fn assume_unused_suppresses_warning() {
        let input = "\
glyph orphan 2 1
@@..

glyph other 2 1
..@@

assume unused orphan
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("glyph 'orphan' is unused")),
            "assume unused should suppress warning: {issues:?}",
        );
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("glyph 'other' is unused")),
            "non-assumed glyph should still be reported: {issues:?}",
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("unrecognized directive")),
            "assume unused should not be flagged as unrecognized: {issues:?}",
        );
    }

    fn group_issues(text: &str) -> Vec<Issue> {
        let doc = document_io::parse_document_from_str(text, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.message.contains("remap group"))
            .collect()
    }

    #[test]
    fn remap_group_ordering_cycle_reported() {
        let issues = group_issues(
            "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap y : a -> a\n\
             remap group x after y\nremap group y after x\n",
        );
        assert_eq!(issues.len(), 2, "one per declaration, got: {issues:?}");
        assert!(
            issues
                .iter()
                .all(|i| i.severity == Severity::Error && i.message.contains("ordering cycle")),
            "got: {issues:?}",
        );
    }

    #[test]
    fn remap_group_after_undefined_group_reported() {
        let issues = group_issues(
            "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap group x after nope\n",
        );
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("undefined group 'nope'")),
            "got: {issues:?}",
        );
    }

    #[test]
    fn remap_group_declared_twice_reported() {
        let issues = group_issues(
            "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap group x\nremap group x\n",
        );
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("declared more than once")),
            "got: {issues:?}",
        );
    }

    #[test]
    fn remap_group_without_rules_reported() {
        let issues = group_issues("remap group lonely\n");
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Warning && i.message.contains("has no rules")),
            "got: {issues:?}",
        );
    }

    /// A `feature` may be written above every rule of the group it attaches;
    /// the check used to depend on scan order and would call that undefined.
    #[test]
    fn feature_may_precede_the_rules_of_its_group() {
        let issues = group_issues(
            "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nmap A = a\nmap B = b\n\
             feature ccmp for DFLT : late\nremap late : a -> b\n",
        );
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn reversed_group_with_a_non_single_rule_reported() {
        let issues = group_issues(
            "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nglyph c = pix\n\
             map A = a\nmap B = b\nmap C = c\n\
             remap x : a -> b\nremap x : a b -> c\nremap group x reversed\n",
        );
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("reversed")
                && i.message.contains("one glyph")),
            "got: {issues:?}",
        );
    }

    /// The same rule is perfectly fine in a group that is not reversed.
    #[test]
    fn a_ligature_is_only_rejected_when_the_group_is_reversed() {
        let issues = group_issues(
            "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nglyph c = pix\n\
             map A = a\nmap B = b\nmap C = c\n\
             remap x : a b -> c\nremap group x\n",
        );
        assert!(issues.is_empty(), "got: {issues:?}");
    }
}
