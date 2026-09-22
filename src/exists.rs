//! `exists PATTERN` — the inverse of a name pattern: a *search* over the glyph
//! names the source already declares, which then states what to build for each
//! one it finds. The directive as an author sees it is in `doc/reference.md`;
//! this is why it is shaped so.
//!
//! [`crate::pattern`] goes forwards: a block states the list of names it
//! declares. That is the wrong way round when the list is not the point but a
//! *condition* on an existing name is — *"wherever a `han-XXXX:15x16` was
//! drawn, make the `han-XXXX` that uses it"*. Written forwards that means
//! enumerating twenty thousand code points twice over and discarding all but
//! the few hundred that were drawn.
//!
//! # One run per match
//!
//! The scoped item is expanded **once per match**, with every `$N` bound to one
//! string. It is not expanded once with each slot bound to the whole list of
//! matches: that made a slot an ordinary [`crate::pattern`] group, combining
//! with the other groups on the line by the largest-cycles rule, so
//! `glyph han-($1)-(g|h|t)` over three matches wrote three names rather than
//! nine. Since a slot's value is whatever a name happened to match, correlating
//! it with an unrelated alternation is very nearly always a mistake, so the
//! search unrolls and the groups beside it mean what they mean everywhere else.
//! [`Scope::rebind`] is the one-slot-one-string shape the build and the
//! specimen both consume.
//!
//! # What is searched
//!
//! Only names a `glyph` **header** declares — aliases ([`crate::alias`])
//! included, because an alias is a name a `ref` may use like any other, and the
//! glyph built from it has to be built. On-demand names ([`crate::on_demand`])
//! never match: they are an infinite set, and a rule that answered "yes" for
//! names nobody wrote would make an `exists` declare glyphs out of thin air.
//!
//! Two matched names that turn out to be **one glyph** are not a case of their
//! own. This once was an error, on the reasoning that the block below would
//! then declare one glyph twice; that measured the wrong thing on both sides.
//! Two matches collide when the *captures* fail to tell them apart, whether or
//! not they alias — `part-(a)(\.0)?` over `part-a` and `part-a.0` binds `$1` to
//! `a` twice — and that is a duplicate declaration like any other, reported
//! where it happens (`issues/remap.rs`) instead of here.
//!
//! # Scope
//!
//! An `exists` binds **the item on the very next line**, and anything else
//! there is an error rather than a wider or narrower reach. Letting it govern a
//! run of items would have to answer where the run ends, which is exactly the
//! question `editor::folding` and `app::rename` already answer for a glyph
//! block; a second answer is how those drift apart. One consequence: `exists`
//! does not stack, so `$N` is never ambiguous about which pattern it came from.
//!
//! A multi-alias (`glyph NAME* = PREFIX*`, [`crate::alias`]) is the one search
//! not written on a line of its own: it scopes its own item, and for the same
//! reason an `exists` cannot scope it in turn.
//!
//! # Recursion
//!
//! An `exists` may match names another `exists` declared, so the bindings are a
//! fixpoint. What it may *not* do is feed itself, directly or through others:
//! that has no least fixpoint, it just grows. Forbidding self-match would catch
//! only the direct case, so the iteration is bounded instead: `n` directives
//! form a DAG of depth at most `n`, so a fixpoint that has not settled after
//! `n` rounds is a cycle. That is [`ExistsCycle`], and it fails the build rather
//! than truncating — a truncated fixpoint is a font whose contents depend on a
//! round count.
//!
//! # The pattern
//!
//! A regular expression, implicitly anchored, over the glyph-name alphabet Σ
//! ([`crate::pattern::is_glyph_name_char`]) rather than over Unicode.
//!
//! Σ constrains the *definition*: every character written into a pattern — a
//! literal, a class member, the end of a range — has to be in Σ, since one that
//! is not can only be a mistake ([`WrittenText`]). It does not constrain the
//! *meaning* by computing over Unicode and then refusing whatever reached
//! outside Σ. That once rejected every negated class, `.`, `\w` and even
//! `(?i)k` (whose case folding holds the Kelvin sign), all of which mean
//! something perfectly good over Σ. So a class is intersected with Σ instead
//! ([`restrict`]) — `[^:]` is every name character but `:` — and one left
//! empty is an error, because it can never match. Anchors and word boundaries
//! are rejected, since the match is always the whole name.

use std::sync::LazyLock;

use regex::Regex;
use regex_syntax::ast::{self, Ast, ClassSetItem};
use regex_syntax::hir::{
    self, Class, ClassBytes, ClassBytesRange, ClassUnicode, ClassUnicodeRange, Hir, HirKind, Look,
};

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
    /// The literal text every match has to start with, read off the parsed
    /// form ([`literal_prefix`]). A necessary condition, never a sufficient
    /// one, and empty when the pattern begins with anything but a literal.
    ///
    /// This is a prefilter, and the reason it exists is the shape of the
    /// search: the fixpoint runs *every* directive over *every* declared name,
    /// so a source with eighty `exists han-96e8\.3:…` lines and sixty thousand
    /// names is millions of match attempts, nearly all of which the first byte
    /// already answers. The regex engine has its own prefilter and still costs
    /// a call to reach it; `starts_with` is the same answer for a memcmp.
    prefix: String,
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
        let hir = restricted_hir(source)
            .map_err(|e| format!("invalid exists pattern `{source}`: {e}"))?;
        let captures = count_captures(&hir);
        if captures > MAX_CAPTURES {
            return Err(format!(
                "invalid exists pattern `{source}`: {captures} capture groups (max {MAX_CAPTURES})"
            ));
        }
        // Anchored by construction, so a pattern is never quietly a substring
        // test. `\A`/`\z` rather than `^`/`$` because the latter are line
        // anchors under multi-line mode and a glyph name is not a line.
        // Compiled from the restricted form rather than `source`, which differs
        // from it in every class.
        let re = Regex::new(&format!(r"\A(?:{hir})\z"))
            .map_err(|e| format!("invalid exists pattern `{source}`: {e}"))?;
        Ok(Self {
            source: source.to_string(),
            re,
            captures,
            prefix: literal_prefix(&hir),
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
        name.starts_with(&self.prefix) && self.re.is_match(name)
    }

    /// `[$0, $1, …]` for a matching name, `None` otherwise.
    ///
    /// A group that took part in no alternative contributes an empty string
    /// rather than dropping out, so the slot count is the pattern's and a `$N`
    /// never silently shifts to another group's value.
    pub fn capture(&self, name: &str) -> Option<Vec<String>> {
        if !name.starts_with(&self.prefix) {
            return None;
        }
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

/// `source` as a regular expression over the glyph-name alphabet, or why it is
/// not one. [`ExistsPattern::parse`] and [`template_denotes`] both read a
/// pattern through here, so navigation never disagrees with the build.
///
/// Two passes, because the two halves of the rule live at different levels.
/// What was *written* is only in the syntax tree: `[^:]` and a class spelling
/// out every other scalar are the same `Class` once translated. What a class
/// *means* is only in the translation, after negation, `&&`, `--` and `(?i)`
/// have been applied — and intersecting with Σ there commutes with all of them,
/// `(U ∖ A) ∩ Σ` being `Σ ∖ A`.
fn restricted_hir(source: &str) -> Result<Hir, String> {
    let ast = ast::parse::Parser::new()
        .parse(source)
        .map_err(|e| syntax_error(&e))?;
    ast::visit(&ast, WrittenText)?;
    let hir = hir::translate::Translator::new()
        .translate(source, &ast)
        .map_err(|e| syntax_error(&e))?;
    restrict(hir)
}

/// The parser's own message spans several lines — a banner, the pattern, a
/// caret rule, then the finding — which is unreadable inside a one-line
/// diagnostic. The finding is the last line, and it is the only part that is
/// not already on screen.
fn syntax_error(e: &dyn std::fmt::Display) -> String {
    let msg = e.to_string();
    let last = msg
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("syntax error");
    last.strip_prefix("error: ").unwrap_or(last).to_string()
}

/// Rejects a character written into the pattern that no glyph name contains.
struct WrittenText;

impl ast::Visitor for WrittenText {
    type Output = ();
    type Err = String;

    fn finish(self) -> Result<(), String> {
        Ok(())
    }

    fn visit_pre(&mut self, ast: &Ast) -> Result<(), String> {
        match ast {
            Ast::Literal(lit) => written(lit.c),
            _ => Ok(()),
        }
    }

    fn visit_class_set_item_pre(&mut self, item: &ClassSetItem) -> Result<(), String> {
        match item {
            ClassSetItem::Literal(lit) => written(lit.c),
            ClassSetItem::Range(range) => written(range.start.c).and(written(range.end.c)),
            _ => Ok(()),
        }
    }
}

fn written(c: char) -> Result<(), String> {
    if crate::pattern::is_glyph_name_char(c) {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a glyph-name character",
            c.escape_debug()
        ))
    }
}

/// Σ as a class of either kind. It is ASCII
/// ([`crate::pattern::is_glyph_name_char`]), so scanning ASCII builds all of it.
static ALPHABET: LazyLock<(ClassUnicode, ClassBytes)> = LazyLock::new(|| {
    let members = || (0..=0x7f_u8).filter(|&b| crate::pattern::is_glyph_name_char(char::from(b)));
    (
        ClassUnicode::new(members().map(|b| ClassUnicodeRange::new(b.into(), b.into()))),
        ClassBytes::new(members().map(|b| ClassBytesRange::new(b, b))),
    )
});

/// `hir` with every class intersected with Σ, and the nodes an `exists` has no
/// use for rejected.
fn restrict(hir: Hir) -> Result<Hir, String> {
    Ok(match hir.into_kind() {
        HirKind::Empty => Hir::empty(),
        // Written text, which `WrittenText` has already held to Σ.
        HirKind::Literal(lit) => Hir::literal(lit.0),
        HirKind::Class(mut class) => {
            match &mut class {
                Class::Unicode(c) => c.intersect(&ALPHABET.0),
                Class::Bytes(c) => c.intersect(&ALPHABET.1),
            }
            if class.is_empty() {
                return Err(
                    "a character class has no glyph-name character in it, so it never matches"
                        .to_string(),
                );
            }
            Hir::class(class)
        }
        // `\b`, `^`, `$`, `\A`, `\z` — the match is the whole name, so an
        // anchor is either redundant or a lie about what is being matched.
        HirKind::Look(look) => return Err(format!("`{}` is not allowed here", look_name(look))),
        HirKind::Repetition(hir::Repetition {
            min,
            max,
            greedy,
            sub,
        }) => Hir::repetition(hir::Repetition {
            min,
            max,
            greedy,
            sub: Box::new(restrict(*sub)?),
        }),
        HirKind::Capture(hir::Capture { index, name, sub }) => Hir::capture(hir::Capture {
            index,
            name,
            sub: Box::new(restrict(*sub)?),
        }),
        HirKind::Concat(subs) => {
            Hir::concat(subs.into_iter().map(restrict).collect::<Result<_, _>>()?)
        }
        HirKind::Alternation(subs) => {
            Hir::alternation(subs.into_iter().map(restrict).collect::<Result<_, _>>()?)
        }
    })
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

/// The literal text a match is bound to start with, or `""` when the pattern
/// starts with anything else.
///
/// Only the leading run of literals is read, and only through the nodes that
/// cannot change what comes first — a capture around the start, the first
/// branch of a concatenation. A repetition, a class or an alternation ends it,
/// since none of them pins a first byte down. `(?i)` never reaches here as a
/// literal: `regex_syntax` turns a case-insensitive letter into a class.
fn literal_prefix(hir: &Hir) -> String {
    fn push(hir: &Hir, out: &mut String) -> bool {
        match hir.kind() {
            HirKind::Literal(lit) => match str::from_utf8(&lit.0) {
                Ok(text) => {
                    out.push_str(text);
                    true
                }
                // Not reachable for a restricted pattern (a literal is written
                // text, held to Σ), and stopping is the safe answer anyway.
                Err(_) => false,
            },
            HirKind::Capture(cap) => push(&cap.sub, out),
            HirKind::Concat(subs) => {
                for sub in subs {
                    if !push(sub, out) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }
    let mut out = String::new();
    push(hir, &mut out);
    out
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

use crate::hash::{HashMap, HashSet};
#[cfg(feature = "editor")]
use std::path::{Path, PathBuf};

use crate::document::{Document, DocumentItem, GlyphName};
use crate::pattern::{NamePartsMap, NamePattern, substitute_name_parts};
use crate::resolve::{Diagnostic, ItemRef};

/// What one `exists` found: its matches, in the order the matched names were
/// declared.
///
/// Match-major (`matches[i]` is one name's `[$0, $1, …]`), which is the shape
/// every consumer wants: the scoped item is expanded once per match, so a
/// match is the unit of work and never a column of a table.
#[derive(Debug, Clone)]
pub struct Scope {
    pub pattern: String,
    pub matches: Vec<Vec<String>>,
    /// `$N` slots the pattern has, `$0` included — the same for every match.
    pub slots: usize,
}

impl Scope {
    /// How many times the scoped item runs.
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// `$0`…`$N` of match `i` written into a map the caller already filled with
    /// the base, over whatever the previous match left there.
    ///
    /// The bindings ride on the [`NamePartsMap`] rather than on a substitution
    /// stage of their own because they are the same thing a `name-parts` is: a
    /// name standing for a list of names — a list of exactly one here, which is
    /// what keeps a slot from combining with the groups written beside it.
    ///
    /// It takes that map rather than returning one because a search over a
    /// font's worth of han glyphs matches tens of thousands of names, and the
    /// base a match is bound over is every `name-parts` the source declares:
    /// cloning that per match dwarfs the binding itself, so a caller unrolling
    /// a whole line clones once for the line and rebinds per match. Every slot
    /// the scope has is written on every call, so no match can read a slot the
    /// last one left behind.
    pub fn rebind(&self, out: &mut NamePartsMap, i: usize) {
        for slot in 0..self.slots {
            out.insert(format!("${slot}"), vec![self.matches[i][slot].clone()]);
        }
    }

    /// Undo every [`Scope::rebind`] on `out`, which was `base` before them:
    /// each slot goes back to what `base` holds for it, or away. What lets a
    /// caller unrolling many scopes keep one copy of the base for all of them.
    pub fn unbind(&self, out: &mut NamePartsMap, base: &NamePartsMap) {
        for slot in 0..self.slots {
            let key = format!("${slot}");
            match base.get(&key) {
                Some(values) => {
                    out.insert(key, values.clone());
                }
                None => {
                    out.remove(&key);
                }
            }
        }
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

    /// Every search and the item it scopes. Only [`FirstMatches`] walks the
    /// whole table, so this follows it behind the `editor` feature.
    #[cfg(feature = "editor")]
    pub fn iter(&self) -> impl Iterator<Item = (ItemRef, &Scope)> {
        self.scoped.iter().map(|(r, s)| (*r, s))
    }

    /// Run `f` once for every way this item's names expand: once with `base`
    /// when no `exists` governs it, and once per match — each `$N` bound to one
    /// string — when one does.
    ///
    /// Not called at all when the item stands for nothing: it *is* the `exists`
    /// line, or the search above it found nothing. Every pass that walks the
    /// source rather than the expansion has to go through here, because the
    /// alternative is reading `han-($1)` as a name.
    ///
    /// A closure rather than a list of maps because the base is every
    /// `name-parts` the source declares and a han search matches tens of
    /// thousands of names: the base is copied once for the whole walk (see
    /// [`Bindings`]) and rebound per match.
    pub fn for_each_binding(
        &self,
        bindings: &mut Bindings<'_>,
        r: ItemRef,
        mut f: impl FnMut(&NamePartsMap),
    ) {
        if self.is_directive(r) {
            return;
        }
        match self.scope(r) {
            None => f(bindings.base),
            Some(scope) if scope.matches.is_empty() => {}
            Some(scope) => {
                let base = bindings.base;
                let bound = bindings.bound.get_or_insert_with(|| base.clone());
                for i in 0..scope.len() {
                    scope.rebind(bound, i);
                    f(bound);
                }
                scope.unbind(bound, base);
            }
        }
    }
}

/// The base a walk binds `$N` over, and the one copy of it the walk rebinds.
///
/// A walk over the source asks [`ExistsScopes::for_each_binding`] about every
/// item, and every multi-alias is a scoped item: a font writing six hundred of
/// them copied every `name-parts` it declares six hundred times per walk, in
/// each of the several walks one rebuild makes. The copy is taken the first
/// time a scoped item is met and put back after each one, so it is the base
/// again whenever it is not in use.
pub struct Bindings<'a> {
    base: &'a NamePartsMap,
    bound: Option<NamePartsMap>,
}

impl<'a> Bindings<'a> {
    pub fn new(base: &'a NamePartsMap) -> Self {
        Self { base, bound: None }
    }
}

/// The first match of every search, keyed by the file and the item the search
/// scopes.
///
/// The editor draws a glyph block *as written* rather than expanding it, so a
/// block under an `exists` has to draw some one of the names the search found,
/// and the first match is that one — for the same reason a pattern block draws
/// its first expansion. Only the first is kept: carrying a han search's tens of
/// thousands of matches into the editor's derived data would cost what
/// [`Scope::rebind`] exists to avoid, and nothing but the drawing reads them.
///
/// Keyed by path and item index rather than by [`ItemRef`] because the editor
/// holds one document at a time and never the slice the refs were numbered
/// against. Both halves are stale the moment the document is edited, exactly
/// as the resolved glyphs beside them are; the next rebuild settles it.
#[cfg(feature = "editor")]
#[derive(Debug, Default, Clone)]
pub struct FirstMatches {
    per_file: HashMap<PathBuf, HashMap<usize, Vec<String>>>,
}

#[cfg(feature = "editor")]
impl FirstMatches {
    pub fn collect(docs: &[&Document], scopes: &ExistsScopes) -> Self {
        let mut per_file: HashMap<PathBuf, HashMap<usize, Vec<String>>> = HashMap::default();
        for (r, scope) in scopes.iter() {
            let Some(first) = scope.matches.first() else {
                continue;
            };
            let Some(doc) = docs.get(r.doc as usize) else {
                continue;
            };
            per_file
                .entry(doc.path.clone())
                .or_default()
                .insert(r.item as usize, first.clone());
        }
        Self { per_file }
    }

    /// `$0`…`$N` of the first match of the search scoping item `item` of
    /// `path`, or `None` where no search does.
    pub fn get(&self, path: &Path, item: usize) -> Option<&[String]> {
        Some(self.per_file.get(path)?.get(&item)?.as_slice())
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
        /// The name the scoped item declares, when it declares one: a `glyph`
        /// header or an alias's own name. Those are what feed names back into
        /// the search; a `map` declares nothing.
        declares: Option<GlyphName>,
        /// The target as written (`PREFIX*`) when this is a multi-alias, which
        /// is its own `origin` and `target` and has no pattern as written.
        multi: Option<String>,
    }
    let mut pending: Vec<Pending> = Vec::new();

    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let origin = ItemRef::new(doc_idx, item_idx);
            let pattern = match item {
                DocumentItem::Exists { pattern, .. } => pattern,
                DocumentItem::GlyphAlias {
                    name,
                    search_prefix: Some(prefix),
                    ..
                } => {
                    let multi = format!("{prefix}*");
                    match ExistsPattern::parse(&crate::alias::multi_alias_search(prefix)) {
                        Ok(pattern) => pending.push(Pending {
                            origin,
                            target: origin,
                            pattern,
                            declares: Some(name.clone()),
                            multi: Some(multi),
                        }),
                        Err(message) => {
                            diagnostics.push(Diagnostic::error(
                                origin,
                                format!("multi-alias `{multi}` cannot be searched: {message}"),
                            ));
                            silenced.push(origin);
                        }
                    }
                    continue;
                }
                _ => continue,
            };
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
                // An alias declares a name exactly as a header does — `glyph
                // part-($1) = ($0)` is how a search gives every drawing it
                // found a second name — so it feeds the search back the same
                // way, and the name it declares is the one on its left.
                Some(
                    DocumentItem::Glyph { name, .. }
                    | DocumentItem::GlyphAlias {
                        name,
                        search_prefix: None,
                        ..
                    },
                ) => Some(name.clone()),
                Some(DocumentItem::Map { .. }) => None,
                other => {
                    fail(
                        &mut diagnostics,
                        format!(
                            "`exists` must be followed on the very next line by a `glyph` block, \
                             an alias or a `map`, not {}",
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
                multi: None,
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
    let mut seen: HashSet<String> = HashSet::default();
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
    for (doc_idx, doc) in docs.iter().enumerate() {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::GlyphAlias { name, .. } = item else {
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
    // Both halves of a round are *incremental*, and that is what keeps the
    // fixpoint from costing the whole search once per round. `names` and each
    // scope's `matches` only ever grow, so a name a directive has already been
    // run over cannot answer differently later, and a match already fed back
    // cannot declare anything new. Every run therefore takes at least two
    // rounds — one that finds the matches, one that confirms nothing follows
    // from them — and re-searching sixty thousand names for that confirmation
    // was half the cost of the whole stage.
    //
    // The two indices are per directive rather than shared because `names`
    // grows *during* a round: a directive earlier in the list has seen fewer
    // names than one after it, and the next round picks up exactly what each
    // one missed.
    //
    // `searched` counts names for a directive with no literal prefix and
    // entries of its prefix's bucket for one with a prefix; both only grow in
    // the order `names` does, so the window means the same either way.
    let mut searched = vec![0usize; pending.len()];
    let mut fed_back = vec![0usize; pending.len()];
    let mut index = PrefixIndex::new(pending.iter().map(|p| p.pattern.prefix.as_str()));
    for (i, name) in names.iter().enumerate() {
        index.add(i, name);
    }
    // One copy of the base for the whole fixpoint rather than one per
    // directive: a source with hundreds of multi-aliases is hundreds of
    // directives, and each directive puts back the slots it bound.
    let mut bound = name_parts.clone();
    for _ in 0..=budget {
        let mut changed = false;
        for (k, p) in pending.iter().enumerate() {
            searched[k] = match index.bucket(&p.pattern.prefix) {
                Some(bucket) => {
                    for &i in &bucket[searched[k]..] {
                        if let Some(caps) = p.pattern.capture(&names[i]) {
                            scopes[k].matches.push(caps);
                            changed = true;
                        }
                    }
                    bucket.len()
                }
                None => {
                    for name in &names[searched[k]..] {
                        if let Some(caps) = p.pattern.capture(name) {
                            scopes[k].matches.push(caps);
                            changed = true;
                        }
                    }
                    names.len()
                }
            };
            let Some(header) = &p.declares else {
                continue;
            };
            if fed_back[k] == scopes[k].len() {
                continue;
            }
            // Per match, because that is how the block below runs: a name
            // the search may go on to find is one the *build* declares, and
            // the build expands the header once for each match with the slots
            // bound to one string each.
            for i in fed_back[k]..scopes[k].len() {
                scopes[k].rebind(&mut bound, i);
                for n in expand_header_names(header, &bound) {
                    if seen.insert(n.clone()) {
                        index.add(names.len(), &n);
                        names.push(n);
                        changed = true;
                    }
                }
            }
            scopes[k].unbind(&mut bound, name_parts);
            fed_back[k] = scopes[k].len();
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
                .map(|p| {
                    p.multi
                        .clone()
                        .unwrap_or_else(|| p.pattern.source().to_string())
                })
                .collect(),
        };
        for p in &pending {
            diagnostics.push(Diagnostic::error(p.origin, cycle.to_string()));
            silenced.push(p.target);
        }
        register_silenced(&mut out, &silenced);
        return (out, diagnostics);
    }

    for (p, scope) in pending.iter().zip(scopes) {
        if scope.matches.is_empty() {
            // Not an error: a source that has not drawn any `han-XXXX:15x16`
            // yet is a source in progress, and the line it would build is
            // simply not built. But it is worth saying, because the pattern
            // that matches nothing looks exactly like the one that works.
            let message = match &p.multi {
                Some(written) => format!(
                    "multi-alias `{written}` matches no declared glyph name, so it names nothing"
                ),
                None => format!(
                    "`exists {}` matches no declared glyph name, so the line below builds nothing",
                    scope.pattern,
                ),
            };
            diagnostics.push(Diagnostic::new(
                crate::issues::Severity::Warning,
                Some(p.origin),
                message,
            ));
        }
        out.scoped.insert(p.target, scope);
    }
    register_silenced(&mut out, &silenced);
    (out, diagnostics)
}

/// The searched names bucketed by the literal prefixes the directives start
/// with ([`ExistsPattern::prefix`]).
///
/// The prefilter answers a name with one memcmp, but it still asks every
/// directive about every name, and a multi-alias is a directive: a font that
/// writes six hundred `glyph han-XXXX.0:* = han-XXXX:*` lines is six hundred
/// scans of sixty thousand names per round, on every rebuild. Bucketed, a
/// directive only reads the names that could match it, and a name is filed by
/// looking up one slice per distinct prefix *length*, of which there are few.
///
/// A bucket holds indices into the searched list in the order they were added,
/// which is the order a scan would have met them in — so the matches, and
/// everything built from them, come out in the same order.
struct PrefixIndex {
    lens: Vec<usize>,
    buckets: HashMap<String, Vec<usize>>,
}

impl PrefixIndex {
    fn new<'a>(prefixes: impl Iterator<Item = &'a str>) -> Self {
        let mut lens = Vec::new();
        let mut buckets: HashMap<String, Vec<usize>> = HashMap::default();
        for prefix in prefixes.filter(|p| !p.is_empty()) {
            if !lens.contains(&prefix.len()) {
                lens.push(prefix.len());
            }
            buckets.entry(prefix.to_string()).or_default();
        }
        Self { lens, buckets }
    }

    fn add(&mut self, idx: usize, name: &str) {
        for &len in &self.lens {
            // `get` is `None` past the end and inside a multi-byte character,
            // neither of which a prefix can match.
            if let Some(head) = name.get(..len)
                && let Some(bucket) = self.buckets.get_mut(head)
            {
                bucket.push(idx);
            }
        }
    }

    /// The indices of the names starting with `prefix`, or `None` for the
    /// empty prefix, which every name starts with and no bucket is kept for.
    fn bucket(&self, prefix: &str) -> Option<&[usize]> {
        if prefix.is_empty() {
            return None;
        }
        self.buckets.get(prefix).map(Vec::as_slice)
    }
}

/// The names one `glyph`/`glyph … =` header declares, with `name_parts`
/// substituted and the pattern expanded.
///
/// This has to be the rule the build declares by: a name the search can find
/// but the build does not declare would make an `exists` build a glyph out of a
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
        Some(DocumentItem::MapDecomposed { .. }) => "a `map generate`",
        Some(DocumentItem::GlyphAlias {
            search_prefix: Some(_),
            ..
        }) => "a multi-alias, which is a search of its own",
        Some(_) => "another directive",
    }
}

/// Whether a `glyph` line is the alias form (`glyph NAME = TARGET`) rather
/// than a block header. Read off the tokens, so a `=` inside a quoted name is
/// not one — the same rule [`crate::document_io`] parses by.
fn is_alias_line(line: &str) -> bool {
    let Ok(tokens) = crate::document_io::tokenize_tokens(line) else {
        return false;
    };
    tokens.iter().skip(2).any(|t| t == "=")
}

/// The pattern of an `exists` line, if `line` is one.
///
/// Text, not the item model: the editor's search and navigation read files they
/// have never parsed — an unopened one comes from the directory snapshot — so
/// the question has to be answerable from the line as written.
pub fn pattern_on_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("exists") {
        return None;
    }
    let tokens = crate::document_io::tokenize_tokens(trimmed).ok()?;
    match tokens.as_slice() {
        [kw, pattern] if kw == "exists" => Some(pattern.clone()),
        _ => None,
    }
}

/// The search a multi-alias line runs, if `line` is one. Text, for the reason
/// [`pattern_on_line`] is.
fn multi_alias_search_on_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("glyph") || !trimmed.contains('*') {
        return None;
    }
    let tokens = crate::document_io::tokenize_tokens(trimmed).ok()?;
    match tokens.as_slice() {
        [kw, name, eq, target, ..] if kw == "glyph" && eq == "=" => {
            let (_, prefix) = crate::alias::multi_alias_prefixes(name, target).ok()??;
            Some(crate::alias::multi_alias_search(prefix))
        }
        _ => None,
    }
}

/// Whether the `glyph` header `template`, governed by `exists pattern`,
/// declares `name`.
///
/// Answered by turning the two written lines into one regular expression: each
/// `($N)` on the header becomes the sub-pattern of that capture group, and
/// everything around it becomes a literal. What that regex accepts is exactly
/// the set of names the header can produce over *all* strings the search could
/// match — so it is an over-approximation of what this source declares, since
/// only the names actually drawn are searched.
///
/// Over-approximating is the right side to err on here: a search that lists a
/// line which turns out to declare nothing costs a click, where one that hides
/// the only line declaring a name costs the name.
///
/// `None` when the two do not combine into a test at all — an unparsable
/// pattern, or a `$N` past the groups it has.
#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
pub fn template_denotes(pattern: &str, template: &str, name: &str) -> Option<bool> {
    let (re, _) = template_regex(pattern, template)?;
    Some(re.is_match(name))
}

/// The `$N` slots the header `template` under `exists pattern` binds where it
/// declares `name` — `[$0, $1, …]`, `None` for a slot the header does not write
/// and the pattern does not pin down.
///
/// The inverse of the expansion, for the one caller that has a declared name and
/// wants the match behind it: navigation, following the `goto` ref of a
/// pattern-written block (`glyph han-($1)` over `ref ($0)`). Where the build
/// knows the match and derives the name, this knows the name and derives the
/// match, so the two agree without navigation waiting on a resolve.
///
/// `$0` — the whole matched name, which is what a `ref ($0)` stands for — is
/// rarely written on the header, and is then rebuilt from the pattern with the
/// slots the header *did* write: `han-` + `$1` + `:15x16`. That is exact when
/// everything the pattern writes outside its groups is literal text; where it is
/// not, `$0` is the one slot left unbound, there being nothing to rebuild the
/// rest of the name from.
///
/// `None` where [`template_denotes`] is `None`, and where the header does not
/// declare `name` at all.
#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
pub fn template_captures(pattern: &str, template: &str, name: &str) -> Option<Vec<Option<String>>> {
    let (re, groups) = template_regex(pattern, template)?;
    let caps = re.captures(name)?;
    let mut values: Vec<Option<String>> = (0..groups.len())
        .map(|slot| caps.name(&slot_group(slot)).map(|m| m.as_str().to_string()))
        .collect();
    if values.first().is_some_and(Option::is_none) {
        let mut whole = String::new();
        // A pattern this cannot rebuild leaves `$0` unbound rather than
        // failing: a block whose `ref` never writes `$0` does not need it.
        if render_match(&restricted_hir(pattern).ok()?, &values, &mut whole).is_some() {
            values[0] = Some(whole);
        }
    }
    Some(values)
}

/// The name a match produced, rebuilt from the slot values the header bound.
/// `None` where the pattern writes anything but literal text and capture groups
/// at its top level, which no slot value can stand in for.
fn render_match(hir: &Hir, values: &[Option<String>], out: &mut String) -> Option<()> {
    match hir.kind() {
        HirKind::Empty => {}
        HirKind::Literal(lit) => out.push_str(std::str::from_utf8(&lit.0).ok()?),
        // The outermost group wins: its value is the text of everything inside
        // it, nested groups included.
        HirKind::Capture(cap) => {
            out.push_str(values.get(usize::try_from(cap.index).ok()?)?.as_deref()?)
        }
        HirKind::Concat(subs) => {
            for sub in subs {
                render_match(sub, values, out)?;
            }
        }
        _ => return None,
    }
    Some(())
}

/// The regex name the first appearance of slot `N` on a header is captured as.
fn slot_group(slot: usize) -> String {
    format!("s{slot}")
}

/// The header `template`, read under `exists pattern`, as one regular
/// expression over glyph names, together with the pattern's group sub-patterns
/// by index (`[0]` is the whole pattern).
///
/// Each `($N)` on the header becomes the sub-pattern of that capture group, and
/// everything around it becomes a literal. What the regex accepts is exactly
/// the set of names the header can produce over *all* strings the search could
/// match — so it is an over-approximation of what this source declares, since
/// only the names actually drawn are searched.
///
/// Over-approximating is the right side to err on here: a search that lists a
/// line which turns out to declare nothing costs a click, where one that hides
/// the only line declaring a name costs the name.
///
/// The first appearance of each slot is a group named [`slot_group`], so a
/// caller can read back what it matched; a repeated slot is not, the regex
/// crate having no back-reference to hold the two together.
///
/// `None` when the two lines do not combine into a test at all — an unparsable
/// pattern, or a `$N` past the groups it has.
fn template_regex(pattern: &str, template: &str) -> Option<(Regex, Vec<String>)> {
    let hir = restricted_hir(pattern).ok()?;
    // `$0` is the whole pattern; `$N` is the group the regex parser gave index
    // `N`, which is the one the author counted opening parentheses to.
    let mut indexed: Vec<(u32, String)> = Vec::new();
    collect_capture_sources(&hir, &mut indexed);
    indexed.sort_by_key(|(i, _)| *i);
    let mut groups: Vec<String> = vec![hir.to_string()];
    for (i, src) in indexed {
        if usize::try_from(i) != Ok(groups.len()) {
            return None;
        }
        groups.push(src);
    }

    let mut out = String::from(r"\A");
    let mut named = vec![false; groups.len()];
    let bytes = template.as_bytes();
    let mut i = 0;
    let mut literal = String::new();
    while i < bytes.len() {
        // `($N)` and a bare `$N` both stand for the slot; the parenthesized
        // form is what a source writes, since that is where a name pattern puts
        // an alternation and the slot sits in one.
        let (slot, width) = match (
            bytes[i],
            bytes.get(i + 1),
            bytes.get(i + 2),
            bytes.get(i + 3),
        ) {
            (b'(', Some(b'$'), Some(d), Some(b')')) if d.is_ascii_digit() => {
                (usize::from(d - b'0'), 4)
            }
            (b'$', Some(d), _, _) if d.is_ascii_digit() => (usize::from(d - b'0'), 2),
            _ => {
                literal.push(template[i..].chars().next()?);
                i += template[i..].chars().next()?.len_utf8();
                continue;
            }
        };
        out.push_str(&literal_regex(&literal));
        literal.clear();
        let sub = groups.get(slot)?;
        if std::mem::replace(named.get_mut(slot)?, true) {
            out.push_str(&format!("(?:{sub})"));
        } else {
            out.push_str(&format!("(?<{}>{sub})", slot_group(slot)));
        }
        i += width;
    }
    out.push_str(&literal_regex(&literal));
    out.push_str(r"\z");
    Some((Regex::new(&out).ok()?, groups))
}

/// What the written text *between* two capture slots accepts.
///
/// Not a literal, because the rest of the header is still a name pattern: the
/// `font/` shape is `glyph han-5b50-($han-regions):($1) = ($0)`, where `($1)`
/// is the search's slot and `($han-regions)` — already substituted to
/// `(g|h|t|…)` by the caller — is an ordinary alternation the header expands
/// itself. Escaping the whole run would make that group match the parenthesis
/// and the bars, so a name it declares would be found nowhere; expanding it
/// here is what lets a click on one of those glyphs reach the line that
/// declares it.
///
/// A run that is not a pattern expands to itself, and one too wide to be worth
/// enumerating (or that does not parse at all) falls back to the literal —
/// which is what this did for every run before.
fn literal_regex(literal: &str) -> String {
    /// Enough for a region list or a variant set. Past it the alternation
    /// costs more than the answer is worth, and the literal — an
    /// under-approximation now rather than an over-approximation — is what is
    /// left.
    const MAX_ALTERNATIVES: usize = 256;

    if !crate::pattern::is_name_pattern(literal) {
        return regex::escape(literal);
    }
    let Ok(pattern) = crate::pattern::NamePattern::parse_segments(literal) else {
        return regex::escape(literal);
    };
    if pattern.is_empty() || pattern.len() > MAX_ALTERNATIVES {
        return regex::escape(literal);
    }
    let alternatives: Vec<String> = pattern.iter().map(|name| regex::escape(&name)).collect();
    format!("(?:{})", alternatives.join("|"))
}

/// Every capture group's own sub-pattern, with the index the parser gave it.
fn collect_capture_sources(hir: &Hir, out: &mut Vec<(u32, String)>) {
    match hir.kind() {
        HirKind::Capture(cap) => {
            out.push((cap.index, cap.sub.to_string()));
            collect_capture_sources(&cap.sub, out);
        }
        HirKind::Repetition(rep) => collect_capture_sources(&rep.sub, out),
        HirKind::Concat(subs) | HirKind::Alternation(subs) => {
            subs.iter().for_each(|h| collect_capture_sources(h, out))
        }
        _ => {}
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

/// How far an `exists` reaches down a file, carried one source line at a time.
///
/// The scope is one item, but an item is several *lines* — a `glyph` block's
/// `ref`, IDC and pixel rows all belong to it, and `ref ($0)` is where the
/// search's own matches are named. So the carry is a small state machine rather
/// than a flag, stepped *before* each line is read: what governs a line has to
/// be known while reading it, and whether the block ended is decided by the
/// line itself ([`crate::document_io::starts_item`]).
///
/// Text again, not the item model, for the reason [`pattern_on_line`] is.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Carry {
    #[default]
    None,
    /// The previous line was the directive; the line being entered is the one
    /// it governs, and it is not yet known whether that is a block or a line.
    Armed(String),
    /// A `map` or an alias: governed for this line and no further. A
    /// multi-alias line enters this directly, being its own search.
    Once(String),
    /// Inside the `glyph` block it governs.
    Body(String),
}

#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
impl Carry {
    /// The pattern in force on the line just entered.
    pub fn pattern(&self) -> Option<&str> {
        match self {
            Carry::None => None,
            Carry::Armed(p) | Carry::Once(p) | Carry::Body(p) => Some(p),
        }
    }

    /// Step onto `line`, which is about to be read.
    ///
    /// A pixel row need not be stepped and must not be stepped as a blank one:
    /// it is inside the block, and it is neither a directive nor the start of
    /// the next item, so the state it would pass through is the state it is in.
    pub fn enter(&mut self, line: &str) {
        if let Some(pattern) = pattern_on_line(line) {
            *self = Carry::Armed(pattern);
            return;
        }
        if let Some(pattern) = multi_alias_search_on_line(line) {
            *self = Carry::Once(pattern);
            return;
        }
        let trimmed = line.trim_start();
        let starts_item = trimmed
            .split_ascii_whitespace()
            .next()
            .is_some_and(crate::document_io::starts_item);
        *self = match std::mem::take(self) {
            // An alias shares the `glyph` keyword with a block and is one line,
            // like a `map` — so it is what it says on that line and nothing
            // below it is governed.
            Carry::Armed(p) if trimmed.starts_with("glyph") && !is_alias_line(trimmed) => {
                Carry::Body(p)
            }
            Carry::Armed(p) => Carry::Once(p),
            Carry::Once(_) => Carry::None,
            Carry::Body(p) if starts_item || trimmed.is_empty() => {
                let _ = p;
                Carry::None
            }
            other => other,
        };
    }
}
