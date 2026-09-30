//! Wide `map` lines with alternatives: which alternative each row settles on, and the pairs that come out.

use std::sync::Arc;

use super::map_spec::{WideMapRows, wide_map_rows};
use super::*;
use crate::pattern::substitute_captures;

mod index;
mod wide;

pub(crate) use index::MapAlternativeIndex;
use wide::{emit_settled_rows, settle_wide_groups};

/// One alternative target of a wide `map` line, ready to be read at a row index.
pub(super) enum AltTarget<'a> {
    /// `PREFIX($-N)SUFFIX`, with nothing else in the target that a pattern
    /// reads: the name at index `i` is the group's `i`-th value between the two
    /// literals. Worth an arm of its own because it is what every ordered-
    /// alternative line in the font writes, and because the general path splices
    /// the whole group into one string and parses it straight back out — per
    /// alternative, on a group tens of thousands of values long.
    Capture {
        prefix: &'a str,
        group: &'a [String],
        suffix: &'a str,
        /// Which capture group of the spec `group` is, for
        /// [`MapAlternativeIndex`] — the one reader that needs to name it.
        group_idx: usize,
    },
    Pattern(NamePattern),
    /// A target no pattern can be made of, read as the literal it is written
    /// as — which is what [`expand_glyph_pattern`](super::map_spec::expand_glyph_pattern) does with one.
    Literal(String),
}

impl<'a> AltTarget<'a> {
    fn of(glyph: &'a str, spec: &'a WideMapRows) -> Self {
        if let Some(target) = Self::capture_only(glyph, spec) {
            return target;
        }
        let substituted = substitute_captures(glyph, spec.captures());
        match NamePattern::parse_element(&substituted) {
            Ok(pattern) => Self::Pattern(pattern),
            Err(_) => Self::Literal(substituted),
        }
    }

    /// The `PREFIX($-N)SUFFIX` form, or `None` when anything else in the target
    /// — another group, another variable, a repeat — means the general path has
    /// to read it.
    fn capture_only(glyph: &'a str, spec: &'a WideMapRows) -> Option<Self> {
        let plain = |s: &str| !s.contains(['(', ')', '|', '*', '$']);
        let (prefix, rest) = glyph.split_once("($-")?;
        let (digits, suffix) = rest.split_once(')')?;
        if !plain(prefix) || !plain(suffix) {
            return None;
        }
        let n: usize = digits.parse().ok()?;
        let idx = n.checked_sub(1)?;
        if !spec.plain(idx) {
            return None;
        }
        Some(Self::Capture {
            prefix,
            group: &spec.captures()[idx],
            suffix,
            group_idx: idx,
        })
    }

    /// The name at expansion index `i`, cyclically — the same rule
    /// [`NamePattern::get`] indexes by.
    fn get(&self, i: usize) -> String {
        let mut out = String::new();
        self.get_into(i, &mut out);
        out
    }

    /// The same, into a buffer the caller reuses. A check that only looks each
    /// name up asks for millions of them and keeps almost none.
    fn get_into(&self, i: usize, out: &mut String) {
        out.clear();
        match self {
            Self::Capture {
                prefix,
                group,
                suffix,
                ..
            } => {
                out.push_str(prefix);
                out.push_str(&group[i % group.len()]);
                out.push_str(suffix);
            }
            Self::Pattern(pattern) => out.push_str(&pattern.get(i)),
            Self::Literal(s) => out.push_str(s),
        }
    }
}

/// Which alternative one row settles on, and whether anything matched at all.
///
/// The first usable alternative wins; where none is, an optional line (one
/// whose last alternative is the empty token) drops the row and any other line
/// takes `.notdef` — if the source declares one. The names are pulled lazily,
/// so a row that settles on its first alternative never builds the other eight.
///
/// The narrow forms' path. A wide line goes through [`settle_alt`] instead,
/// which answers with an index into the alternatives rather than a name.
fn settle_row(
    names: impl Iterator<Item = String>,
    usable: &impl Fn(&str) -> bool,
    optional: bool,
) -> (Option<String>, bool) {
    for name in names {
        if usable(&name) {
            return (Some(name), true);
        }
    }
    if optional {
        return (None, true);
    }
    (usable(NOTDEF).then(|| NOTDEF.to_string()), false)
}

/// Every alternative of one `map` line, expanded over the line's characters.
///
/// The same pairs [`expand_map_pairs`] returns for each alternative in turn,
/// but with the character spec — which a range line writes tens of thousands of
/// characters wide — expanded once for the line rather than once per
/// alternative. For a caller that needs every alternative's name per row and so
/// cannot stream them ([`for_each_map_alternative_name`]) or read them lazily
/// (`resolve_map_alternatives`): the specimen, which asks its own oracle which
/// one a character settles on.
#[cfg(feature = "editor")]
pub(crate) fn expand_map_pairs_per_alternative(
    char_repr: &str,
    glyphs: &[String],
) -> Vec<Vec<(u32, String)>> {
    let Some(spec) = wide_map_rows(char_repr) else {
        return glyphs
            .iter()
            .map(|g| expand_map_pairs(char_repr, g))
            .collect();
    };
    glyphs
        .iter()
        .map(|g| {
            let target = AltTarget::of(g, &spec);
            spec.rows()
                .iter()
                .map(|&(i, cp)| (cp, target.get(i)))
                .collect()
        })
        .collect()
}

/// Every glyph name one `map` line's alternatives put on a character, one at a
/// time.
///
/// A source-side check only wants to look each name up, and a range line nine
/// alternatives deep names millions of them: streaming through one buffer is
/// what keeps that from being millions of `String`s the caller drops again.
pub(crate) fn for_each_map_alternative_name(
    char_repr: &str,
    glyphs: &[String],
    index: &mut MapAlternativeIndex,
    mut f: impl FnMut(&str),
) {
    let Some(spec) = wide_map_rows(char_repr) else {
        for glyph in glyphs {
            for (_, name) in expand_map_pairs(char_repr, glyph) {
                f(&name);
            }
        }
        return;
    };
    let mut buf = String::new();
    for glyph in glyphs {
        let target = AltTarget::of(glyph, &spec);
        if index.take(&target, spec.spec.as_ref()) {
            continue;
        }
        for &(i, _) in spec.rows() {
            target.get_into(i, &mut buf);
            f(&buf);
        }
    }
}

/// The one diagnostic a line's unmatched characters produce, reported once for
/// the line rather than once per character: a range fails the same way for
/// every character it covers.
fn report_unmatched(
    unmatched: Option<(String, usize)>,
    glyphs: &[String],
    origin: Option<ItemRef>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((first, more)) = unmatched else {
        return;
    };
    let listed = glyphs
        .iter()
        .filter(|g| !g.is_empty())
        .map(|g| format!("'{g}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let and_more = match more {
        0 => String::new(),
        n => format!(" (and {n} more character{})", if n == 1 { "" } else { "s" }),
    };
    diagnostics.push(Diagnostic::error(
        origin,
        format!("map '{first}'{and_more} has no target: none of {listed} names a glyph"),
    ));
}

/// Which alternative one row of a wide `map` line settles on.
///
/// An index rather than the name itself: the name is rebuilt from the line's
/// targets where the row is emitted, which costs the one allocation the item
/// needs anyway, and keeps a hundred thousand of them out of the settling.
#[derive(Clone, Copy)]
enum SettledAlt {
    /// Index into the line's alternatives.
    Alt(u16),
    /// Nothing matched on a line whose last alternative is the empty token:
    /// the row maps nothing, and that is what the line asked for.
    Dropped,
    /// Nothing matched, and the source declares a `.notdef` to fall back on.
    Notdef,
    /// Nothing matched and there is nothing to fall back on.
    Nothing,
}

/// [`settle_row`] over one row of a wide line's `count` alternatives.
///
/// `usable_at(n)` answers for the line's `n`-th alternative at this row. It is
/// the name read off the target and judged for a line settling alone, and the
/// group's per-row memo for one settling with others; either way the caller
/// builds the name into a buffer of its own, because a row that settles on its
/// ninth alternative would otherwise allocate nine names.
fn settle_alt(
    count: usize,
    mut usable_at: impl FnMut(usize) -> bool,
    optional: bool,
    notdef_usable: bool,
) -> SettledAlt {
    for n in 0..count {
        if usable_at(n) {
            return SettledAlt::Alt(n as u16);
        }
    }
    if optional {
        return SettledAlt::Dropped;
    }
    if notdef_usable {
        SettledAlt::Notdef
    } else {
        SettledAlt::Nothing
    }
}

/// Pick each `map`'s target out of the alternatives it lists.
///
/// `map CHAR = first second` means *first if it exists, otherwise second*, and
/// the choice is per **codepoint**, not per line: a target is a name pattern
/// expanded in lock-step with `char_repr`, so `map U+($#4e00..9fff) = a-($-1)
/// b-($-1)` asks the question eighteen thousand times and may well answer it
/// differently each time. That is why the line cannot simply keep a pattern of
/// its own — there is no one winner to keep — and why a multi-target line is
/// *split* here into one single-target `map` per codepoint. Everything
/// downstream (the cmap collectors, the sample, GSUB's variation-sequence pass)
/// then reads the ordinary one-target item it always read, and cannot disagree
/// with this pass about which glyph a character got.
///
/// The split is why the pass returns immediately for source that lists no
/// alternatives anywhere: it costs a walk of `all_items` and nothing else, and
/// every source written before this syntax existed takes that path.
///
/// `.notdef` is the implicit last alternative, as it is in the font itself: a
/// character that matched nothing draws the missing-glyph box rather than
/// borrowing whichever name happened to be written last. If the source does not
/// declare `.notdef` either the codepoint is left unmapped, which reaches a
/// renderer as the same thing — glyph id 0.
///
/// Reported once per line rather than once per codepoint: a range line fails
/// the same way for every character it covers, and the specimen is where the
/// per-character answer belongs (see [`crate::specimen`]).
///
/// # The empty target
///
/// A last alternative written as the empty token — `` `` `` — says that a
/// character matching none of the others is *not an error*: the mapping is
/// dropped, silently, and the character is simply not in the font. That is the
/// difference between "this range should be covered and something is missing"
/// and "this range is covered as far as it goes", and only the source knows
/// which it meant, so it has to be written down.
///
/// It has to be *last*, because everything after a target that always matches
/// would be unreachable — and it always matches, being the one alternative that
/// asks for no glyph at all. An empty token anywhere else is reported by
/// [`crate::issues`], which reads the line as written; here it simply names no
/// glyph, like any other name that stands for nothing.
pub(super) fn resolve_map_alternatives(
    all_items: &mut Vec<ExpandedItem>,
    aliases: &crate::alias::AliasMap,
    diagnostics: &mut Vec<Diagnostic>,
    cancel: &crate::cancel::CancelToken,
) {
    // An empty target changes what a *single*-target line means too, so it is
    // enough on its own to bring the line through this pass.
    let resolvable = |e: &ExpandedItem| {
        matches!(&e.item, DocumentItem::Map { glyphs, .. }
            if glyphs.len() > 1 || glyphs.iter().any(String::is_empty))
    };
    if !all_items.iter().any(resolvable) {
        return;
    }

    // The same two sets `inject_on_demand_glyph_items` builds, and for the same
    // reason: a name is only worth mapping if a glyph is going to be built for
    // it, which a contentless block is not. Built here as well as there because
    // `expand_decomposed_maps` adds glyphs between the two passes.
    let mut defined: HashSet<String> = HashSet::default();
    let mut contentless: HashSet<String> = HashSet::default();
    for e in all_items.iter() {
        match &e.item {
            DocumentItem::Glyph {
                name: GlyphName(n),
                body,
            } => {
                defined.insert(n.clone());
                if body.pixels.is_none() && body.refs.is_empty() && !body.keep {
                    contentless.insert(n.clone());
                }
            }
            // `map generate` runs *after* this pass — it needs the resolved
            // maps to find the components it decomposes into — but the names it
            // will declare are already readable off the line, and an
            // alternative naming one is naming a glyph that is going to be
            // there.
            DocumentItem::MapDecomposed {
                char_repr, glyph, ..
            } => {
                for (_, name) in decomposed_map_pairs(char_repr, glyph.as_deref()) {
                    defined.insert(name);
                }
            }
            _ => {}
        }
    }
    // An alternative may name an alias, or a shape the font generates on
    // demand; both are glyphs the character would really get, so both count as
    // present. Canonicalized first, for the same reason the pairs are.
    let usable = |name: &str| {
        if name.is_empty() {
            // The "map nothing" target names no glyph, ever: it is read where
            // the alternatives run out, not as one of them.
            return false;
        }
        let canon = aliases.resolved_target(name).unwrap_or(name);
        if defined.contains(canon) {
            return !contentless.contains(canon);
        }
        crate::on_demand::detect_on_demand_glyph(canon, |n| {
            defined.contains(aliases.resolved_target(n).unwrap_or(n))
        })
        .is_some()
    };

    // Asked once for the whole pass rather than once per row that runs out of
    // alternatives: `usable` is pure, and this is the one name every such row
    // asks about.
    let notdef_usable = usable(NOTDEF);

    let items = std::mem::take(all_items);
    // A cancelled settling leaves rows unsettled, which the emitting below
    // skips; the caller then discards the whole expansion.
    let mut grouped = settle_wide_groups(&items, &resolvable, &usable, notdef_usable, cancel);

    let mut out: Vec<ExpandedItem> = Vec::with_capacity(items.len());
    for (idx, e) in items.into_iter().enumerate() {
        if !resolvable(&e) {
            out.push(e);
            continue;
        }
        let origin = e.origin;
        let DocumentItem::Map {
            slices,
            char_repr,
            selector,
            glyphs,
            ..
        } = e.item
        else {
            unreachable!("matched just above");
        };

        // The written form, not an expanded one: an empty pattern expands to
        // an empty name, so this is the same question either way.
        let optional = glyphs.last().is_some_and(String::is_empty);
        // The wide path below takes its own from `emit_settled_rows`; this one
        // is the narrow path's, accumulated as it walks.
        let mut unmatched: Option<(String, usize)> = None;
        // Where a wide line settles alone, its rows live here — beside the
        // borrow of a group's, so the two read the same way below.
        let settled_alone;

        // A wide line expands its character spec once and reads every
        // alternative off it by index; see [`WideMapRows`]. A line whose spec
        // expands to nothing keeps its shape, so the checks downstream still
        // see the line they are written to report.
        if let Some(spec) = selector
            .is_none()
            .then(|| wide_map_rows(&char_repr))
            .flatten()
        {
            if spec.rows().is_empty() {
                out.push(ExpandedItem {
                    item: DocumentItem::Map {
                        slices,
                        char_repr,
                        selector,
                        glyphs,
                        comment: None,
                    },
                    origin,
                });
                continue;
            }
            // Settled with the other lines that write this same character
            // spec, where there were any; see [`settle_wide_groups`].
            let group = grouped.remove(&idx);
            let spec = group.as_ref().map_or(&spec, |g| &g.spec);
            let targets: Vec<AltTarget<'_>> =
                glyphs.iter().map(|g| AltTarget::of(g, spec)).collect();
            let rows = spec.rows();
            // One row is a name built per alternative and a lookup each, and a
            // range line is a hundred thousand of them — the largest single
            // stage of an expansion, and a pure one: `usable` reads sets
            // nothing here writes. Settled on every core, then walked in order,
            // because the items and the diagnostic are the line's and have to
            // come out in the order it wrote them.
            let settled = match &group {
                Some(group) => &group.settled,
                None => {
                    settled_alone = crate::parallel::map_indexed(rows.len(), cancel, |k| {
                        let (i, _) = rows[k];
                        let mut buf = String::new();
                        settle_alt(
                            targets.len(),
                            |n| {
                                targets[n].get_into(i, &mut buf);
                                usable(&buf)
                            },
                            optional,
                            notdef_usable,
                        )
                    });
                    &settled_alone
                }
            };
            let unmatched = emit_settled_rows(rows, settled, &targets, &slices, origin, &mut out);
            report_unmatched(unmatched, &glyphs, origin, diagnostics);
            continue;
        }

        // `(codepoint spelling, selector spelling, one name per alternative)`.
        // Both forms are reduced to this so the choice below is written once.
        let rows: Vec<(String, Option<String>, Vec<String>)> = match &selector {
            Some(sel) => {
                let per_alt: Vec<Vec<(u32, u32, String)>> = glyphs
                    .iter()
                    .map(|g| expand_uvs_map_triples(&char_repr, sel, g).unwrap_or_default())
                    .collect();
                let first = &per_alt[0];
                (0..first.len())
                    .map(|i| {
                        (
                            format!("U+{:04X}", first[i].0),
                            Some(format!("U+{:04X}", first[i].1)),
                            per_alt
                                .iter()
                                .filter_map(|alt| alt.get(i).map(|t| t.2.clone()))
                                .collect(),
                        )
                    })
                    .collect()
            }
            None => {
                let per_alt: Vec<Vec<(u32, String)>> = glyphs
                    .iter()
                    .map(|g| expand_map_pairs(&char_repr, g))
                    .collect();
                let first = &per_alt[0];
                (0..first.len())
                    .map(|i| {
                        (
                            format!("U+{:04X}", first[i].0),
                            None,
                            per_alt
                                .iter()
                                .filter_map(|alt| alt.get(i).map(|p| p.1.clone()))
                                .collect(),
                        )
                    })
                    .collect()
            }
        };

        if rows.is_empty() {
            out.push(ExpandedItem {
                item: DocumentItem::Map {
                    slices,
                    char_repr,
                    selector,
                    glyphs,
                    comment: None,
                },
                origin,
            });
            continue;
        }

        for (cp_repr, sel_repr, names) in rows {
            let (chosen, matched) = settle_row(names.into_iter(), &usable, optional);
            if !matched {
                match &mut unmatched {
                    Some((_, more)) => *more += 1,
                    None => unmatched = Some((cp_repr.clone(), 0)),
                }
            }
            let Some(chosen) = chosen else { continue };
            out.push(ExpandedItem {
                item: DocumentItem::Map {
                    slices: slices.clone(),
                    char_repr: cp_repr,
                    selector: sel_repr,
                    glyphs: vec![chosen],
                    comment: None,
                },
                origin,
            });
        }

        report_unmatched(unmatched, &glyphs, origin, diagnostics);
    }
    *all_items = out;
}
