//! `exists PATTERN` — the inverse of a name pattern: a *search* over the glyph
//! names the source already declares, which then states what to build for each
//! one it finds.
//!
//! [`crate::pattern`] goes forwards. A block carries the list of every name it
//! declares, and `ifexists` prunes the expansions that turned out to name
//! nothing. That reads backwards when the list is not the point: `glyph
//! han-($#4e00..9fff)` with `ref han-($#4e00..9fff):15x16 ifexists` writes
//! 20,992 names twice over to say *"wherever a `han-XXXX:15x16` was drawn, make
//! the `han-XXXX` that uses it"*. `exists` says that instead:
//!
//! ```text
//! exists han-([0-9a-f]{4,5}):15x16
//! glyph han-($1) 16 16 advance 16
//! ref ($0)
//! ```
//!
//! `$0` is the whole matched name and `$1`… its capture groups, usable anywhere
//! the scoped item takes a name pattern. There is no `ifexists` on either line:
//! the search *is* the condition, and it is exact where `ifexists` is not —
//! see [`crate::render::ttf_builder::expand::declared_glyph_names`] for the
//! one-round over-approximation an `ifexists` `map` is answered against.
//!
//! # What is searched
//!
//! Only names a `glyph` **header** declares. Two exclusions follow from that
//! and both are deliberate:
//!
//! - **On-demand names** ([`crate::on_demand`]) never match. They are an
//!   infinite set — every `WxH`, every `-polyN` — so a search cannot enumerate
//!   them, and a rule that answered "yes" for names nobody wrote would make an
//!   `exists` declare glyphs out of thin air. This is where `exists` is
//!   narrower than `ifexists`, which does accept them.
//! - **Aliases** ([`crate::alias`]) *are* searched, because an alias is a name
//!   a `ref` may use like any other — `glyph han-4ee4:15x16 = han-4ee4-k:15x16`
//!   is how a source gives one regional variant the plain name, and the glyph
//!   built from it has to be built. What is refused is narrower and is the real
//!   hazard: two matched names that are **the same glyph**. Then `$0` has two
//!   values for one glyph, and the block below would declare it twice or map a
//!   codepoint to it twice, and nothing here can say which name was meant.
//!
//! # Scope
//!
//! An `exists` binds **the item on the very next line** — one `glyph` block
//! (its `ref`/IDC/grid lines included) or one `map`. A blank line, a comment or
//! anything else there is an error rather than a wider or narrower reach. The
//! alternative, letting it govern a run of items, would have to answer where
//! the run ends, which is exactly the question `editor::folding` and
//! `app::rename` already answer for a glyph block; giving `exists` a second
//! answer to it is how those two drift apart.
//!
//! One consequence worth stating: `exists` does not stack, so `$N` is never
//! ambiguous about which pattern it came from.
//!
//! # Recursion
//!
//! An `exists` may match names another `exists` declared — that is what makes
//! it composable — so the bindings are a fixpoint, not one round. What it may
//! *not* do is feed itself, directly or through others: `exists a-(…)`
//! declaring `b-…` while `exists b-(…)` declares `a-…` has no least fixpoint,
//! it just grows.
//!
//! Forbidding self-match would catch only the direct case, so instead the
//! iteration is bounded: a set of `n` `exists` directives forms a DAG of depth
//! at most `n`, so a fixpoint that has not settled after `n` rounds is a cycle.
//! That is [`ExistsCycle`], and it fails the build rather than truncating —
//! a truncated fixpoint is a font whose contents depend on a round count.
//!
//! # The pattern
//!
//! A regular expression, implicitly anchored at both ends, restricted to a
//! subset by [`check_subset`]: literals, character classes, repetition, groups
//! and alternation. Every literal and every class must be within the glyph-name
//! character set ([`crate::pattern::is_valid_glyph_name`]'s), which is what
//! rejects a bare `.` — `.` matches `(` and `|` and would let a match carry
//! pattern syntax into a name. A literal dot is `\.`, and `.` stays available
//! as the ordinary name character it is everywhere else in a `.unf`.
//!
//! Anchors and word boundaries are rejected for the same reason a name pattern
//! has none: the match is the whole name, always.

use regex::Regex;
use regex_syntax::hir::{Hir, HirKind, Look};

/// The most capture groups an `exists` may have: `$1`…`$9`, since `$0` is the
/// whole name and a two-digit `$10` would not be distinguishable from `$1`
/// followed by a `0` in a name like `han-($1)0`.
pub const MAX_CAPTURES: usize = 9;

/// A compiled `exists` pattern.
#[derive(Debug, Clone)]
pub struct ExistsPattern {
    source: String,
    re: Regex,
    captures: usize,
}

impl PartialEq for ExistsPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for ExistsPattern {}

impl ExistsPattern {
    /// Compile `source`, rejecting anything outside the subset.
    pub fn parse(source: &str) -> Result<Self, String> {
        if source.is_empty() {
            return Err("exists pattern is empty".to_string());
        }
        let hir = regex_syntax::parse(source).map_err(|e| {
            // The parser's own message spans several lines — a banner, the
            // pattern, a caret rule, then the finding — which is unreadable
            // inside a one-line diagnostic. The finding is the last line, and
            // it is the only part that is not already on screen.
            let msg = e.to_string();
            let last = msg
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .next_back()
                .unwrap_or("syntax error");
            let last = last.strip_prefix("error: ").unwrap_or(last);
            format!("invalid exists pattern `{source}`: {last}")
        })?;
        check_subset(&hir).map_err(|e| format!("invalid exists pattern `{source}`: {e}"))?;
        let captures = count_captures(&hir);
        if captures > MAX_CAPTURES {
            return Err(format!(
                "invalid exists pattern `{source}`: {captures} capture groups (max {MAX_CAPTURES})"
            ));
        }
        // Anchored by construction, so a pattern is never quietly a substring
        // test. `\A`/`\z` rather than `^`/`$` because the latter are line
        // anchors under multi-line mode and a glyph name is not a line.
        let re = Regex::new(&format!(r"\A(?:{source})\z"))
            .map_err(|e| format!("invalid exists pattern `{source}`: {e}"))?;
        Ok(Self {
            source: source.to_string(),
            re,
            captures,
        })
    }

    /// The pattern as written, which is what a diagnostic names it by.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// How many `$N` the scoped item may use, not counting `$0`.
    pub fn capture_count(&self) -> usize {
        self.captures
    }

    /// Whether `name` is one of the search's matches. Live in the tests below
    /// and nowhere else yet: the editor's navigation is the caller this is for,
    /// and it does not ask the question through here so far.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn is_match(&self, name: &str) -> bool {
        self.re.is_match(name)
    }

    /// `[$0, $1, …]` for a matching name, `None` otherwise.
    ///
    /// A group that took part in no alternative contributes an empty string
    /// rather than dropping out, so the slot count is the pattern's and a `$N`
    /// never silently shifts to another group's value.
    pub fn capture(&self, name: &str) -> Option<Vec<String>> {
        let caps = self.re.captures(name)?;
        Some(
            (0..=self.captures)
                .map(|i| {
                    caps.get(i)
                        .map(|m| m.as_str())
                        .unwrap_or_default()
                        .to_string()
                })
                .collect(),
        )
    }
}

/// Reject the regex features an `exists` has no use for, before one reaches a
/// name.
///
/// The rule is stated over the parsed form rather than the text so that it
/// cannot be spelled around: `.` and `[^x]` and `\w` are one `Class` node each,
/// and all three are answered by the same question — can this match a character
/// a glyph name may not contain?
fn check_subset(hir: &Hir) -> Result<(), String> {
    match hir.kind() {
        HirKind::Empty => Ok(()),
        HirKind::Literal(lit) => match str::from_utf8(&lit.0) {
            Ok(s) if s.chars().all(is_name_char) => Ok(()),
            _ => Err(format!(
                "literal `{}` is not glyph-name text",
                String::from_utf8_lossy(&lit.0)
            )),
        },
        HirKind::Class(class) => {
            let ok = match class {
                regex_syntax::hir::Class::Unicode(c) => c
                    .iter()
                    .all(|r| char_range_is_name_text(r.start(), r.end())),
                regex_syntax::hir::Class::Bytes(c) => c
                    .iter()
                    .all(|r| char_range_is_name_text(r.start() as char, r.end() as char)),
            };
            if ok {
                Ok(())
            } else {
                Err("a character class must stay within glyph-name characters \
                     (write `\\.` for a literal dot; `.`, `\\w` and `[^…]` match too much)"
                    .to_string())
            }
        }
        // `\b`, `^`, `$`, `\A`, `\z` — the match is the whole name, so an
        // anchor is either redundant or a lie about what is being matched.
        HirKind::Look(look) => Err(format!("`{}` is not allowed here", look_name(*look))),
        HirKind::Repetition(rep) => check_subset(&rep.sub),
        HirKind::Capture(cap) => check_subset(&cap.sub),
        HirKind::Concat(subs) | HirKind::Alternation(subs) => {
            subs.iter().try_for_each(check_subset)
        }
    }
}

fn look_name(look: Look) -> &'static str {
    match look {
        Look::Start | Look::StartLF | Look::StartCRLF => "^",
        Look::End | Look::EndLF | Look::EndCRLF => "$",
        Look::WordAscii | Look::WordUnicode => r"\b",
        Look::WordAsciiNegate | Look::WordUnicodeNegate => r"\B",
        _ => "a look-around assertion",
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':')
}

/// Whether every character in an inclusive range is a name character. Ranges
/// are short in practice (`0-9`, `a-f`); a class spanning the whole code space
/// fails on its first character.
fn char_range_is_name_text(start: char, end: char) -> bool {
    if !is_name_char(start) || !is_name_char(end) {
        return false;
    }
    (start..=end).all(is_name_char)
}

fn count_captures(hir: &Hir) -> usize {
    match hir.kind() {
        HirKind::Empty | HirKind::Literal(_) | HirKind::Class(_) | HirKind::Look(_) => 0,
        HirKind::Repetition(rep) => count_captures(&rep.sub),
        HirKind::Capture(cap) => 1 + count_captures(&cap.sub),
        HirKind::Concat(subs) | HirKind::Alternation(subs) => subs.iter().map(count_captures).sum(),
    }
}

/// The `exists` directives of a source could not be brought to a fixpoint: one
/// of them declares a name another one matches, in a cycle.
#[derive(Debug, Clone)]
pub struct ExistsCycle {
    /// The patterns involved, as written — every one still growing when the
    /// round budget ran out. Naming all of them is the point: a cycle has no
    /// single culprit line to point at.
    pub patterns: Vec<String>,
}

impl std::fmt::Display for ExistsCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exists directives feed each other in a cycle: {}",
            self.patterns.join(", ")
        )
    }
}

#[cfg(test)]
#[path = "exists_tests.rs"]
mod exists_tests;

use std::collections::{HashMap, HashSet};

use crate::document::{Document, DocumentItem, GlyphName};
use crate::pattern::{NamePartsMap, NamePattern, substitute_name_parts};
use crate::resolve::{Diagnostic, ItemRef};

/// What one `exists` found: its matches, in the order the matched names were
/// declared.
///
/// Match-major (`matches[i]` is one name's `[$0, $1, …]`) because that is the
/// shape a `map` wants — it becomes one mapping per match — while a `glyph`
/// block wants the transpose, which [`Scope::bindings`] takes.
#[derive(Debug, Clone)]
pub struct Scope {
    pub pattern: String,
    pub matches: Vec<Vec<String>>,
    /// `$N` slots the pattern has, `$0` included — the same for every match.
    pub slots: usize,
}

impl Scope {
    /// `$0`…`$N` bound over `base`, one alternation per slot, for expanding the
    /// scoped item's name patterns in one pass.
    ///
    /// The bindings ride on the [`NamePartsMap`] rather than on a new
    /// substitution stage because they are the same thing a `name-parts` is: a
    /// name that stands for a list. What makes the slots *correlated* — `$0`
    /// and `$1` of one match landing in the same expanded name — is that every
    /// slot binds the same number of values, so [`crate::pattern`]'s cyclic
    /// combination indexes them in lock-step. That is the rule name patterns
    /// already run on; there is nothing new here to get wrong.
    pub fn bindings(&self, base: &NamePartsMap) -> NamePartsMap {
        let mut out = base.clone();
        for slot in 0..self.slots {
            out.insert(
                format!("${slot}"),
                self.matches.iter().map(|m| m[slot].clone()).collect(),
            );
        }
        out
    }

    /// The same for one match alone: what a `map` is stated with, since a
    /// mapping is emitted per matched name rather than once for the line.
    pub fn bindings_for(&self, base: &NamePartsMap, i: usize) -> NamePartsMap {
        let mut out = base.clone();
        for slot in 0..self.slots {
            out.insert(format!("${slot}"), vec![self.matches[i][slot].clone()]);
        }
        out
    }
}

/// Every `exists` of a document set, resolved: which item each one scopes and
/// what it found.
#[derive(Debug, Default, Clone)]
pub struct ExistsScopes {
    /// Keyed by the **scoped** item — the one on the next line — because that
    /// is the item every consumer has in hand while walking a document.
    scoped: HashMap<ItemRef, Scope>,
    /// The `exists` lines themselves. They declare nothing and expand to
    /// nothing; a walk skips them.
    directives: HashSet<ItemRef>,
}

impl ExistsScopes {
    /// Whether this item is an `exists` line (which contributes no item of its
    /// own downstream).
    pub fn is_directive(&self, r: ItemRef) -> bool {
        self.directives.contains(&r)
    }

    /// The search governing this item, if an `exists` is written above it.
    pub fn scope(&self, r: ItemRef) -> Option<&Scope> {
        self.scoped.get(&r)
    }

    pub fn is_empty(&self) -> bool {
        self.directives.is_empty()
    }

    /// The name parts an item's names expand with: the base ones, plus `$N`
    /// when an `exists` governs it.
    ///
    /// `None` means the item declares and refers to nothing at all — either the
    /// search above it found nothing, or the item *is* the `exists` line. Every
    /// pass that walks the source rather than the expansion has to ask this,
    /// because the alternative is reading `han-($1)` as a name.
    pub fn parts_at<'a>(
        &self,
        base: &'a NamePartsMap,
        r: ItemRef,
    ) -> Option<std::borrow::Cow<'a, NamePartsMap>> {
        if self.is_directive(r) {
            return None;
        }
        match self.scope(r) {
            None => Some(std::borrow::Cow::Borrowed(base)),
            Some(scope) if scope.matches.is_empty() => None,
            Some(scope) => Some(std::borrow::Cow::Owned(scope.bindings(base))),
        }
    }
}

/// Resolve every `exists` in `docs` to its matches, and report what cannot be.
///
/// The searched set is grown to a fixpoint: an `exists` may match names another
/// `exists` declared. See the module docs for the round budget that stands in
/// for cycle detection.
pub fn resolve_scopes(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    aliases: &crate::alias::AliasMap,
) -> (ExistsScopes, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut out = ExistsScopes::default();
    // Items whose `exists` failed: registered below with no matches, so every
    // consumer treats them as standing for nothing.
    let mut silenced: Vec<ItemRef> = Vec::new();

    // Every `exists` and the item it scopes, with the pattern compiled once.
    struct Pending {
        origin: ItemRef,
        target: ItemRef,
        pattern: ExistsPattern,
        /// The scoped `glyph` header, when it is one: the only kind of item
        /// that feeds names back into the search.
        declares: Option<GlyphName>,
    }
    let mut pending: Vec<Pending> = Vec::new();

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Exists { pattern, .. } = item else {
                continue;
            };
            let origin = ItemRef::new(doc_idx, item_idx);
            out.directives.insert(origin);
            // A search that cannot run leaves the line below it standing for
            // nothing, rather than for a glyph named `han-($1)`: the `$N` on it
            // is unbindable, and reporting it as a bad name on top of the real
            // error is two findings for one fault.
            let mut fail = |diagnostics: &mut Vec<Diagnostic>, message: String| {
                diagnostics.push(Diagnostic::error(origin, message));
                silenced.push(ItemRef::new(doc_idx, item_idx + 1));
            };
            let compiled = match ExistsPattern::parse(pattern) {
                Ok(p) => p,
                Err(message) => {
                    fail(&mut diagnostics, message);
                    continue;
                }
            };
            // The scope is the next item, and only the next item. Anything else
            // there — a blank line, a comment, a second `exists` — is a line
            // that reads as if it were governed and is not.
            let target = ItemRef::new(doc_idx, item_idx + 1);
            let declares = match doc.items.get(item_idx + 1) {
                Some(DocumentItem::Glyph { name, .. }) => Some(name.clone()),
                Some(DocumentItem::Map { .. }) => None,
                other => {
                    fail(
                        &mut diagnostics,
                        format!(
                            "`exists` must be followed on the very next line by a `glyph` block \
                             or a `map`, not {}",
                            describe_scoped(other),
                        ),
                    );
                    continue;
                }
            };
            pending.push(Pending {
                origin,
                target,
                pattern: compiled,
                declares,
            });
        }
    }

    let register_silenced = |out: &mut ExistsScopes, silenced: &[ItemRef]| {
        for target in silenced {
            out.scoped.entry(*target).or_insert_with(|| Scope {
                pattern: String::new(),
                matches: Vec::new(),
                slots: 1,
            });
        }
    };
    if pending.is_empty() {
        register_silenced(&mut out, &silenced);
        return (out, diagnostics);
    }

    // The names an `exists` may find: written `glyph` headers, and nothing
    // else. Blocks that are themselves scoped are left out of the seed — they
    // have no names until their own search has run.
    let scoped_targets: HashSet<ItemRef> = pending.iter().map(|p| p.target).collect();
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Glyph { name, .. } = item else {
                continue;
            };
            if scoped_targets.contains(&ItemRef::new(doc_idx, item_idx)) {
                continue;
            }
            for n in expand_header_names(name, name_parts) {
                if seen.insert(n.clone()) {
                    names.push(n);
                }
            }
        }
    }

    // An alias is a name a `ref` may use, so it is searchable like any other —
    // a source that names one regional variant `han-4ee4:15x16` means that name
    // to be findable. Aliases declare no glyph of their own, so they are added
    // to the searched set rather than to the seed of what blocks declare.
    for doc in docs {
        for item in &doc.items {
            let DocumentItem::GlyphAlias { name, .. } = item else {
                continue;
            };
            for n in expand_header_names(name, name_parts) {
                if seen.insert(n.clone()) {
                    names.push(n);
                }
            }
        }
    }

    // The fixpoint. A set of `n` `exists` forms a DAG of depth at most `n`, so
    // a round that still changes something after `n` of them is a cycle rather
    // than a slow convergence; see the module docs.
    let mut scopes: Vec<Scope> = pending
        .iter()
        .map(|p| Scope {
            pattern: p.pattern.source().to_string(),
            matches: Vec::new(),
            slots: p.pattern.capture_count() + 1,
        })
        .collect();
    let budget = pending.len();
    let mut settled = false;
    for _ in 0..=budget {
        let mut changed = false;
        for (k, p) in pending.iter().enumerate() {
            let matches: Vec<Vec<String>> =
                names.iter().filter_map(|n| p.pattern.capture(n)).collect();
            if matches.len() != scopes[k].matches.len() {
                changed = true;
            }
            scopes[k].matches = matches;
            let Some(header) = &p.declares else {
                continue;
            };
            if scopes[k].matches.is_empty() {
                continue;
            }
            let bound = scopes[k].bindings(name_parts);
            for n in expand_header_names(header, &bound) {
                if seen.insert(n.clone()) {
                    names.push(n);
                    changed = true;
                }
            }
        }
        if !changed {
            settled = true;
            break;
        }
    }
    if !settled {
        let cycle = ExistsCycle {
            patterns: pending
                .iter()
                .map(|p| p.pattern.source().to_string())
                .collect(),
        };
        for p in &pending {
            diagnostics.push(Diagnostic::error(p.origin, cycle.to_string()));
            silenced.push(p.target);
        }
        register_silenced(&mut out, &silenced);
        return (out, diagnostics);
    }

    for (p, mut scope) in pending.iter().zip(scopes) {
        // Two names of one glyph, both found. `$0` would then have two values
        // for one glyph and the line below would build it twice — or map two
        // code points to it, which is a `map` a source can write on purpose but
        // not one a search may write by accident.
        let mut by_glyph: HashMap<&str, &str> = HashMap::new();
        let mut collision = None;
        for m in &scope.matches {
            let canonical = aliases.resolved_target(&m[0]).unwrap_or(&m[0]);
            if let Some(first) = by_glyph.insert(canonical, &m[0]) {
                collision = Some((first.to_string(), m[0].clone(), canonical.to_string()));
                break;
            }
        }
        if let Some((first, second, glyph)) = collision {
            diagnostics.push(Diagnostic::error(
                p.origin,
                format!(
                    "`exists {}` matches both `{first}` and `{second}`, which are two names \
                     for the glyph `{glyph}`; narrow the pattern so it finds each glyph once",
                    scope.pattern,
                ),
            ));
            scope.matches.clear();
            out.scoped.insert(p.target, scope);
            continue;
        }
        if scope.matches.is_empty() {
            // Not an error: a source that has not drawn any `han-XXXX:15x16`
            // yet is a source in progress, and the line it would build is
            // simply not built. But it is worth saying, because the pattern
            // that matches nothing looks exactly like the one that works.
            diagnostics.push(Diagnostic::new(
                crate::issues::Severity::Warning,
                Some(p.origin),
                format!(
                    "`exists {}` matches no declared glyph name, so the line below builds nothing",
                    scope.pattern,
                ),
            ));
        }
        out.scoped.insert(p.target, scope);
    }
    register_silenced(&mut out, &silenced);
    (out, diagnostics)
}

/// The names one `glyph`/`glyph … =` header declares, with `name_parts`
/// substituted and the pattern expanded.
///
/// This is [`crate::render::ttf_builder::expand::declared_glyph_names`]'s rule
/// for one header, and it has to stay that rule: a name the search can find but
/// the build does not declare would make an `exists` build a glyph out of a
/// name nothing draws.
fn expand_header_names(name: &GlyphName, name_parts: &NamePartsMap) -> Vec<String> {
    let text = substitute_name_parts(&name.display(), name_parts);
    match NamePattern::parse(&text) {
        Ok(pattern) => (0..pattern.len()).map(|i| pattern.get(i)).collect(),
        Err(_) => Vec::new(),
    }
}

fn describe_scoped(item: Option<&DocumentItem>) -> &'static str {
    match item {
        None => "the end of the file",
        Some(DocumentItem::BlankLine) => "a blank line",
        Some(DocumentItem::Comment(_)) => "a comment",
        Some(DocumentItem::Heading { .. }) => "a heading",
        Some(DocumentItem::Exists { .. }) => "another `exists`",
        Some(DocumentItem::GlyphAlias { .. }) => "an alias",
        Some(DocumentItem::MapDecomposed { .. }) => "a `map generate`",
        Some(_) => "another directive",
    }
}

/// Whether a written name mentions a capture slot (`$0`…`$9`), and so names
/// whatever the `exists` above it matched rather than a glyph of its own.
///
/// The slot names are reserved: `name-parts $0` is unwritable, so a `$0` left
/// in a name after substitution can only have come from a search. That is what
/// makes this answerable from the text alone, which is what the editor needs —
/// it underlines an undefined `ref` while typing, long before anything has
/// resolved the searches.
pub fn mentions_capture(name: &str) -> bool {
    name.as_bytes()
        .windows(2)
        .any(|w| w[0] == b'$' && w[1].is_ascii_digit())
}

/// Evaluate the character spelling on the left of an `exists`-scoped `map`.
///
/// `U+[BASE+]($N)` — the capture read as hexadecimal and added to `BASE`
/// (`0` when omitted). Hexadecimal on both sides with no decimal alternative:
/// `U+` has meant hex everywhere else in a `.unf` since there was a `.unf`, and
/// a base that changed with a sigil would be one more thing to read twice.
///
/// A spelling with no `($N)` in it is left exactly as written — the ordinary
/// forms (`U+XXXX`, a literal character, a range) still mean what they always
/// did, and a scoped `map` may use them, though a line that names one codepoint
/// per match maps it once per match and duplicates.
///
/// Both halves of a variation sequence take this, which is what makes
/// `map U+($1) U+E0100+($2)` writable.
pub fn eval_codepoint(spec: &str, caps: &[String]) -> Result<String, String> {
    if !spec.contains('(') {
        return Ok(spec.to_string());
    }
    let rest = spec
        .strip_prefix("U+")
        .or_else(|| spec.strip_prefix("u+"))
        .ok_or_else(|| format!("`{spec}` uses `($N)` but does not start with `U+`"))?;
    let (base_text, slot_text) = match rest.split_once('+') {
        Some((base, slot)) => (base, slot),
        None => ("", rest),
    };
    let base = if base_text.is_empty() {
        0u32
    } else {
        u32::from_str_radix(base_text, 16)
            .map_err(|_| format!("`{spec}`: `{base_text}` is not hexadecimal"))?
    };
    let slot = slot_text
        .strip_prefix("($")
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| {
            format!("`{spec}`: expected `U+[BASE+]($N)` after the base, found `{slot_text}`")
        })?;
    let value = caps
        .get(slot)
        .ok_or_else(|| format!("`{spec}`: the `exists` pattern has no `${slot}`"))?;
    let offset = u32::from_str_radix(value, 16)
        .map_err(|_| format!("`{spec}`: `${slot}` is `{value}`, which is not hexadecimal"))?;
    let cp = base
        .checked_add(offset)
        .filter(|cp| char::from_u32(*cp).is_some())
        .ok_or_else(|| format!("`{spec}`: {base:X}+{offset:X} is not a Unicode code point"))?;
    Ok(format!("U+{cp:04X}"))
}
