//! Glyph-flag consequences that reach past the glyph the flag is written on.

use std::collections::{HashMap, HashSet};

use crate::document::{DocumentItem, GlyphBody, GlyphName, GlyphRef, LayerVisibility};

use super::{Cx, Issue, Severity, issue_at};

/// `vectoronly` is not a property of one glyph but of a *drawing*, so it has
/// to reach everything the flagged glyph pulls in through `ref` — the bitmap
/// flavor squares a grid off into the shared cache, and a composite exempted
/// on its own would still be assembled out of blocks (see
/// `render::ttf_builder::collect::vectoronly_closure`).
///
/// The price is that the component is drawn as vector artwork for *every*
/// glyph in the bitmap face, not only the flagged one. That is silent and
/// surprising where the component is shared, so it is reported: either the
/// sharing glyph wants the flag too, or the two want separate components.
/// Nothing is reported where the exemption reaches only glyphs that exist to
/// serve it, which is the ordinary case.
pub(super) fn check_vectoronly_reach(
    cx: &Cx<'_>,
    mapped_glyphs: &HashSet<String>,
    issues: &mut Vec<Issue>,
) {
    let mut roots: HashSet<&str> = HashSet::new();
    let mut layers_of: HashMap<&str, Option<LayerVisibility>> = HashMap::new();
    let mut refs_of: HashMap<&str, Vec<&GlyphRef>> = HashMap::new();
    for item in cx.expansion.items() {
        let DocumentItem::Glyph {
            name: GlyphName(n),
            body,
        } = item
        else {
            continue;
        };
        if body.vectoronly {
            roots.insert(n.as_str());
            layers_of.insert(n.as_str(), body.vectoronly_layers);
        }
        refs_of.entry(n.as_str()).or_default().extend(&body.refs);
    }
    if roots.is_empty() {
        return;
    }

    // The closure, minus the flagged glyphs themselves: a root is exempt
    // because it was asked to be, and owes no explanation.
    let mut reached: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = roots.iter().copied().collect();
    while let Some(name) = queue.pop() {
        // The same scope rule the build's own closure walks by, so the two
        // agree on what a flagged drawing reaches.
        let layers = layers_of.get(name).copied().flatten();
        for &r in refs_of.get(name).map(Vec::as_slice).unwrap_or(&[]) {
            if !GlyphBody::vectoronly_covers(layers, r) {
                continue;
            }
            let r = r.name.as_str();
            if !roots.contains(r) && reached.insert(r) {
                queue.push(r);
            }
        }
    }
    if reached.is_empty() {
        return;
    }

    // Who else draws one of those, and how. A glyph outside the closure that
    // refs it gets the vector artwork in its own composite; a *mapped* one
    // gets it as the character's own drawing.
    let mut outside_users: HashMap<&str, &str> = HashMap::new();
    for (user, refs) in &refs_of {
        if roots.contains(user) || reached.contains(user) {
            continue;
        }
        for r in refs {
            let r = r.name.as_str();
            if reached.contains(r) {
                outside_users.entry(r).or_insert(user);
            }
        }
    }

    for (doc_idx, doc) in cx.docs.iter().enumerate() {
        for (item_idx, item) in cx.source_items(doc_idx) {
            let DocumentItem::Glyph {
                name: GlyphName(n), ..
            } = item
            else {
                continue;
            };
            let name = n.as_str();
            if !reached.contains(name) {
                continue;
            }
            let message = if let Some(user) = outside_users.get(name) {
                format!(
                    "glyph '{name}' is drawn as vector artwork in the bitmap face because a \
                     `vectoronly` glyph refers to it, but '{user}' refers to it too and did not \
                     ask for that — flag '{user}' as well, or give the two separate components"
                )
            } else if mapped_glyphs.contains(name) {
                format!(
                    "glyph '{name}' is drawn as vector artwork in the bitmap face because a \
                     `vectoronly` glyph refers to it, and it is mapped to a character of its own \
                     — write `vectoronly` on it too if that is what it wants"
                )
            } else {
                continue;
            };
            issues.push(issue_at(doc, item_idx, Severity::Warning, message));
        }
    }
}
