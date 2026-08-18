//! Checks over the glyph names a source declares: what an alias may name,
//! and what characters a name may contain.

use crate::document::{DocumentItem, GlyphName, is_valid_glyph_name};
use crate::resolve::{Diagnostic, ItemRef};

use super::{Cx, Issue};

/// What an alias names has to exist, and the name has to be free.
pub(super) fn check_aliases(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docset = &cx.docset;
    let aliases = cx.aliases;
    let all_glyph_names = &cx.all_glyph_names;

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
            && crate::on_demand::parse_on_demand_glyph(target).is_none()
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
}

/// Names the font will actually carry, checked once against the charset.
pub(super) fn check_glyph_charset(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docset = &cx.docset;
    let expansion = cx.expansion;
    // Names the font will actually carry, checked once against the charset.
    // Against the *expanded* names, so a pattern that produced something odd is
    // caught the same way a hand-written name would be.
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
        // A name that still carries its `@` was written above every glyph
        // the `@` could stand for. Saying so beats the charset wording,
        // which reads as though `@` were simply misspelled.
        let message = if name.starts_with('@') {
            format!(
                "glyph name `{name}` has no glyph to expand `@` against: `@` stands for \
                     the last glyph declared without one, and this file declares none above it",
            )
        } else {
            format!("glyph name `{name}` may only contain letters, digits, `-`, `.`, `_` and `:`",)
        };
        issues.push(docset.to_issue(&Diagnostic::error(origin, message)));
    }
}
