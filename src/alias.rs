//! `glyph NAME = TARGET` — glyph aliases.
//!
//! An alias is a **second name for one glyph**, not a second glyph. `glyph A =
//! B` says that `A` and `B` are the same thing, so everything that names `A` —
//! a `map`, a `ref`, a `remap` operand, an assertion — is treated as if it had
//! named `B`, and the font ends up with one glyph carrying one glyph id.
//!
//! It used to mean `glyph A` + `ref B`: a distinct glyph whose only content was
//! a full-size reference to another. That is a different font — two glyph ids
//! with identical outlines — and in every remaining use in `font/` it was the
//! alias that was meant. The old form also accepted the glyph flags (`keep`,
//! `advance N`, …), which only made sense for a real glyph; they are a parse
//! error now, and a glyph that needs any of them is written in block form with
//! `ref TARGET` instead.
//!
//! An alias is written by hand, but it is not the only thing in the map: the
//! expansions of one `glyph` pattern block that describe the same glyph are
//! folded in as **implicit** merges, which is [`crate::merge`]'s whole output.
//! They differ from a declared alias in exactly one way — the name is also a
//! `glyph` block, whose item the expansion drops in favour of the survivor's
//! ([`AliasMap::is_implicit`]) — and they never join [`AliasMap::decls`],
//! since nothing wrote them. [`AliasMap::collect_with_merges`] is the
//! constructor that includes them, and what reads glyph names as the font will
//! carry them uses it.
//!
//! # How the rest of the pipeline sees it
//!
//! [`AliasMap::collect`] is the only place that reads
//! [`DocumentItem::GlyphAlias`]. It resolves chains (`A = B`, `B = C` means `A`
//! is `C`) once, up front, so every consumer needs a single `canonicalize`
//! call and never a loop. From there, expansion
//! ([`crate::render::ttf_builder::expand_for`]) rewrites every glyph-name
//! reference to its canonical name and drops the alias items, so the glyph
//! cache, the cmap, GSUB and the sample never learn that aliases exist.
//!
//! The two consumers that do not go through expansion — GSUB, which expands
//! `remap` patterns straight from the documents, and `assert shape`, which
//! compares against the built font's glyph names — canonicalize with the same
//! map.
//!
//! One reference keeps both names: an IDC line's component
//! ([`crate::document::ComposeItem::Part`]) is canonicalized like every other,
//! but the name it was written with is kept beside it, because a component's
//! name is also a claim about which slot of the split it fills
//! ([`crate::compose`]'s variant name rule). `阝:4x16-c = 阝:4x16-r` is a
//! source saying the right-hand drawing is what a `⿲`'s middle slot uses, and
//! it is the `-c` that says so; that is also the one thing that makes such a
//! drawing reachable for the middle slot at all, in the check and in
//! [`crate::fix::clearance`]'s variant search alike.
//!
//! One deliberate exception: [`crate::ref_composite::resolve_expansion`] adds
//! the alias names back into the resolved-glyph map after resolution finishes.
//! The editor validates the names it finds *in the text* against that map, and
//! a `ref A` the build resolves perfectly well must not be underlined as
//! undefined. They are added after the alternatives index is built, so an alias
//! never becomes an `x:alt` alternative of anything.
//!
//! # What is an error
//!
//! Declaring one alias name twice, and an alias cycle, are reported here. That
//! the target exists, that the name is not also a `glyph` block, and that the
//! alias is used at all are reported by [`crate::issues`], which is where the
//! full glyph set is known.

use std::collections::{HashMap, HashSet};

use crate::document::{Document, DocumentItem, NamePartsMap, substitute_name_parts};
use crate::pattern::{NamePattern, capture_groups, substitute_captures};
use crate::resolve::{Diagnostic, ItemRef};

/// One `glyph NAME = TARGET` after name-part substitution and pattern
/// expansion, still pointing at whatever it was written as.
#[derive(Clone)]
pub struct AliasDecl {
    pub name: String,
    /// The target as written, before chains are followed.
    pub target: String,
    pub origin: Option<ItemRef>,
}

/// Every glyph alias a document set declares, with chains already followed.
#[derive(Clone, Default)]
pub struct AliasMap {
    /// Alias name → the canonical glyph name it stands for. Never contains a
    /// key that is also a value: chains are collapsed at construction.
    map: HashMap<String, String>,
    /// The declarations as written, in source order — what validation and the
    /// unused-glyph walk report against.
    decls: Vec<AliasDecl>,
    /// The subset of `map`'s keys that no line declares: the implicit merges
    /// of [`crate::merge`]. They differ from a declared alias in one way only
    /// — the name *is* a `glyph` block, whose item the expansion drops in
    /// favour of the survivor's — so that is the one question asked of this.
    implicit: HashSet<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl AliasMap {
    /// Every declared alias, with no `exists` in sight.
    ///
    /// The callers that read aliases beside the *written* source use this —
    /// they read a written glyph block the same way, without binding `$N` — so
    /// an alias a search declares is not in it. Everything that reads glyph
    /// names as the font will carry them goes through
    /// [`collect_with_merges`](Self::collect_with_merges), which does bind.
    pub fn collect(docs: &[&Document], name_parts: &NamePartsMap) -> Self {
        Self::collect_inner(docs, name_parts, None)
    }

    fn collect_inner(
        docs: &[&Document],
        name_parts: &NamePartsMap,
        exists: Option<&crate::exists::ExistsScopes>,
    ) -> Self {
        let mut decls: Vec<AliasDecl> = Vec::new();
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut seen: HashMap<String, Option<ItemRef>> = HashMap::new();

        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let DocumentItem::GlyphAlias { name, target, .. } = item else {
                    continue;
                };
                let origin = Some(ItemRef::new(doc_idx, item_idx));
                // An `exists` above it makes the line one alias per match,
                // each `$N` bound to one string — the same unrolling a scoped
                // `glyph` block gets, which is what lets a search give every
                // drawing it found a second name.
                let mut expanded: Vec<(String, String)> = Vec::new();
                let mut round = 0usize;
                let mut per_binding = |name_parts: &NamePartsMap| {
                    // A pattern the line got wrong is the line's fault, and
                    // every match fails it the same way, so it is reported on
                    // the first run and not once per match.
                    let mut per = Vec::new();
                    expanded.extend(expand_alias(
                        &name.display(),
                        target,
                        name_parts,
                        origin,
                        &mut per,
                    ));
                    if round == 0 {
                        diagnostics.extend(per);
                    }
                    round += 1;
                };
                match exists {
                    Some(exists) => exists.for_each_binding(
                        name_parts,
                        ItemRef::new(doc_idx, item_idx),
                        per_binding,
                    ),
                    None => per_binding(name_parts),
                }
                for (name, target) in expanded {
                    if seen.contains_key(&name) {
                        diagnostics.push(Diagnostic::error(
                            origin,
                            format!("glyph alias `{name}` is declared more than once"),
                        ));
                        continue;
                    }
                    seen.insert(name.clone(), origin);
                    decls.push(AliasDecl {
                        name,
                        target,
                        origin,
                    });
                }
            }
        }

        // Follow chains. Bounded by the number of aliases, so a cycle stops at
        // the first name it revisits rather than spinning.
        let direct: HashMap<&str, &str> = decls
            .iter()
            .map(|d| (d.name.as_str(), d.target.as_str()))
            .collect();
        let mut map: HashMap<String, String> = HashMap::new();
        for decl in &decls {
            let mut cur = decl.target.as_str();
            let mut steps = 0usize;
            let mut cycle = cur == decl.name;
            while let Some(&next) = direct.get(cur) {
                if next == decl.name || steps > direct.len() {
                    cycle = true;
                    break;
                }
                cur = next;
                steps += 1;
            }
            if cycle {
                diagnostics.push(Diagnostic::error(
                    decl.origin,
                    format!(
                        "glyph alias `{}` is in a cycle, so it names no glyph",
                        decl.name,
                    ),
                ));
                continue;
            }
            map.insert(decl.name.clone(), cur.to_string());
        }

        Self {
            map,
            decls,
            implicit: HashSet::new(),
            diagnostics,
        }
    }

    /// [`collect`](Self::collect) plus the merges [`crate::merge`] finds: the
    /// names a `glyph` pattern block declares that describe one glyph between
    /// them. Both kinds of name are the same thing downstream — a name for a
    /// glyph that carries another name — so they share one map.
    ///
    /// The consumers that read glyph names *as the font will carry them* build
    /// the map this way: the expansion, and `assert shape`, which names the
    /// glyphs a shaper produced. Validation reads the same map through the
    /// expansion. What reports against a written line uses
    /// [`decls`](Self::decls), which an implicit merge never joins.
    pub fn collect_with_merges(
        docs: &[&Document],
        name_parts: &NamePartsMap,
        exists: &crate::exists::ExistsScopes,
    ) -> Self {
        let mut aliases = Self::collect_inner(docs, name_parts, Some(exists));
        let merges = crate::merge::implicit_merges(docs, name_parts, &aliases, exists);
        // Declared first: `glyph A = B` is what the author wrote, and an
        // implicit merge is only ever a second opinion about the same name.
        for (name, target) in merges {
            if let std::collections::hash_map::Entry::Vacant(e) = aliases.map.entry(name) {
                aliases.implicit.insert(e.key().clone());
                e.insert(target);
            }
        }
        // A declared alias resolved its chain before the merges were known, so
        // it may still name an expansion that has since been merged away — a
        // name no glyph carries any more. Follow it one step further; a merge
        // target is canonical by construction (`crate::merge` stores a
        // representative, never another key), so one step is the whole chain.
        let survivors: HashMap<String, String> = aliases
            .map
            .iter()
            .filter(|(name, _)| !aliases.implicit.contains(name.as_str()))
            .filter_map(|(name, target)| {
                let merged = aliases.map.get(target.as_str())?;
                if !aliases.implicit.contains(target.as_str()) {
                    return None;
                }
                Some((name.clone(), merged.clone()))
            })
            .collect();
        for (name, target) in survivors {
            aliases.map.insert(name, target);
        }
        aliases
    }

    /// Whether `name` is a name a `glyph` block declares that has been merged
    /// into another of the same block's expansions — the one case where the
    /// name's own glyph item is not the glyph it names.
    pub fn is_implicit(&self, name: &str) -> bool {
        self.implicit.contains(name)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Rewrite `name` in place when it is an alias. A no-op — and no
    /// allocation — for the overwhelmingly common non-alias name.
    pub fn canonicalize(&self, name: &mut String) {
        if self.map.is_empty() {
            return;
        }
        if let Some(target) = self.map.get(name.as_str()) {
            name.clone_from(target);
        }
    }

    pub fn canonicalize_all(&self, names: &mut [String]) {
        if self.map.is_empty() {
            return;
        }
        for name in names {
            self.canonicalize(name);
        }
    }

    /// Canonicalize the glyph names of `(codepoint, glyph)` pairs.
    ///
    /// A `map` target stays a pattern in the expanded item list — a range map
    /// is thousands of codepoints wide, and materializing it into the item
    /// would cost more than every consumer re-expanding it does. So this is
    /// where a mapped alias is resolved: at each `expand_map_pairs` call site,
    /// on the concrete names it produced.
    pub fn canonicalize_pairs(&self, pairs: &mut [(u32, String)]) {
        if self.map.is_empty() {
            return;
        }
        for (_, name) in pairs {
            self.canonicalize(name);
        }
    }

    /// The declarations as written, in source order.
    pub fn decls(&self) -> &[AliasDecl] {
        &self.decls
    }

    /// What `name` resolves to when it is a declared alias that resolved at
    /// all — `None` both for an ordinary glyph name and for an alias dropped
    /// because it is in a cycle.
    pub fn resolved_target(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(|t| t.as_str())
    }

    /// Alias name → canonical target, for the consumers that need to add the
    /// alias names back (the editor's resolved-glyph map).
    pub fn entries(&self) -> impl Iterator<Item = (&String, &String)> {
        self.map.iter()
    }
}

/// Expand one alias declaration's name and target in lock-step, the way a
/// glyph block expands against its `ref` lines: the name pattern decides how
/// many aliases are declared and the target pattern is consumed cyclically.
fn expand_alias(
    name: &str,
    target: &str,
    name_parts: &NamePartsMap,
    origin: Option<ItemRef>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, String)> {
    let name = substitute_name_parts(name, name_parts);
    // The groups the alias's own name wrote, which its target may name with a
    // `$-N` back-reference — the same scope a glyph block gives its `ref`s.
    let captures = capture_groups(&name);
    let target = substitute_captures(&substitute_name_parts(target, name_parts), &captures);

    if !crate::document::is_name_pattern(&name) && !crate::document::is_name_pattern(&target) {
        return vec![(name, target)];
    }

    let name_pattern = match NamePattern::parse(&name) {
        Ok(p) => p,
        Err(e) => {
            diagnostics.push(Diagnostic::error(origin, e.to_string()));
            return Vec::new();
        }
    };
    let target_pattern = match NamePattern::parse_segments(&target) {
        Ok(p) => p,
        Err(e) => {
            diagnostics.push(Diagnostic::error(origin, e.to_string()));
            return Vec::new();
        }
    };
    (0..name_pattern.len())
        .map(|i| (name_pattern.get(i), target_pattern.get(i)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::parse_document_from_str;

    fn collect(src: &str) -> AliasMap {
        let doc = parse_document_from_str(src, "t.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        AliasMap::collect(&docs, &name_parts)
    }

    fn collect_with_merges(src: &str) -> AliasMap {
        let doc = parse_document_from_str(src, "t.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = crate::document::collect_name_parts(&docs);
        let (exists, _) = crate::exists::resolve_scopes(&docs, &name_parts);
        AliasMap::collect_with_merges(&docs, &name_parts, &exists)
    }

    /// The glyph a name stands for, as every consumer sees it: the name itself
    /// unless it is an alias that resolved.
    fn canonical<'a>(aliases: &'a AliasMap, name: &'a str) -> &'a str {
        aliases.resolved_target(name).unwrap_or(name)
    }

    /// The target may name a group of the alias's own name pattern, so the two
    /// stay in lock-step without the alternatives being written twice.
    #[test]
    fn target_back_references_the_name_pattern() {
        let aliases = collect("glyph x-(a|b|c) = y-($-1)\n");
        assert_eq!(canonical(&aliases, "x-a"), "y-a");
        assert_eq!(canonical(&aliases, "x-b"), "y-b");
        assert_eq!(canonical(&aliases, "x-c"), "y-c");
        assert!(aliases.diagnostics.is_empty());
    }

    #[test]
    fn plain_alias() {
        let aliases = collect("glyph a = b\n");
        assert_eq!(canonical(&aliases, "a"), "b");
        assert_eq!(canonical(&aliases, "b"), "b");
        assert!(aliases.diagnostics.is_empty());
    }

    #[test]
    fn chain_collapses_to_the_end() {
        let aliases = collect("glyph a = b\nglyph b = c\n");
        assert_eq!(canonical(&aliases, "a"), "c");
        assert_eq!(canonical(&aliases, "b"), "c");
        assert!(aliases.diagnostics.is_empty());
    }

    #[test]
    fn cycle_is_an_error_and_resolves_to_nothing() {
        let aliases = collect("glyph a = b\nglyph b = a\n");
        assert_eq!(aliases.diagnostics.len(), 2);
        assert!(aliases.diagnostics[0].message.contains("cycle"));
        // Neither name is rewritten, so the reference is reported where it is
        // written rather than silently pointing somewhere arbitrary.
        assert_eq!(canonical(&aliases, "a"), "a");
        assert_eq!(canonical(&aliases, "b"), "b");
    }

    #[test]
    fn self_alias_is_a_cycle() {
        let aliases = collect("glyph a = a\n");
        assert_eq!(aliases.diagnostics.len(), 1);
        assert_eq!(canonical(&aliases, "a"), "a");
    }

    #[test]
    fn duplicate_declaration_is_an_error() {
        let aliases = collect("glyph a = b\nglyph a = c\n");
        assert_eq!(aliases.diagnostics.len(), 1);
        assert!(aliases.diagnostics[0].message.contains("more than once"));
        assert_eq!(canonical(&aliases, "a"), "b");
    }

    #[test]
    fn patterns_expand_in_lock_step() {
        let aliases = collect("glyph x-(a|b|c) = y-(a|b|c)-f\n");
        assert_eq!(canonical(&aliases, "x-a"), "y-a-f");
        assert_eq!(canonical(&aliases, "x-b"), "y-b-f");
        assert_eq!(canonical(&aliases, "x-c"), "y-c-f");
        assert!(aliases.diagnostics.is_empty());
    }

    /// A declared alias naming one expansion of a `glyph` pattern block keeps
    /// naming a glyph after that expansion is merged away: chains are followed
    /// again once the implicit merges are in, or the alias would point at a
    /// name no glyph carries any more.
    #[test]
    fn a_declared_alias_follows_an_implicit_merge() {
        let aliases = collect_with_merges("glyph a-(j|k) 1 1\n@\nglyph b = a-k\n");
        assert_eq!(canonical(&aliases, "a-k"), "a-j");
        assert_eq!(canonical(&aliases, "b"), "a-j");
        assert!(aliases.diagnostics.is_empty());
    }

    #[test]
    fn name_parts_are_substituted_on_both_sides() {
        let aliases = collect("name-parts $v = a b\nglyph x-($v) = y-($v)\n");
        assert_eq!(canonical(&aliases, "x-a"), "y-a");
        assert_eq!(canonical(&aliases, "x-b"), "y-b");
    }
}
