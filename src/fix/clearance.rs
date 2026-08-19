//! `uniform fix --optimize-clearance`: put an IDC line's clearances back inside
//! the range `audit ideal-clearance` states, by choosing among the variants the
//! source already draws and the gaps the line may write.
//!
//! # What it touches
//!
//! **Only a line the source already reports.** A line whose measurements sit in
//! the range is a line whose author has decided something, and rewriting it to
//! a layout this file happens to prefer is churn nobody asked for. The same
//! rule cuts the other way at the end — a rewrite is emitted only when it
//! *lowers* the score, so a line that cannot be improved keeps the warning
//! rather than being shuffled about.
//!
//! There are two kinds of report it acts on, and they are not the same act:
//!
//! - a **clearance warning**, where the line has a layout and the layout is
//!   outside the range. The search moves it inside, and the rewrite has to
//!   lower the score or it is not made;
//! - a **TODO**, where a component has not picked its variant
//!   ([`crate::compose::is_undecided`]). There is no layout at all then, so
//!   there is no score to lower — but the family the component names is on hand
//!   and choosing from it is exactly what the TODO asks for. Such a line is
//!   planned whatever it scores, since any decided layout is more than none,
//!   and its [`ClearanceFix::before`] is `None` to say so.
//!
//! What is skipped is what cannot be measured *after* a choice either: a
//! component that names nothing, a part with no ink of its own, an undecided
//! component whose family is empty. A glyph whose name is a pattern is skipped
//! as well, since one line then stands for a family and the parts of each
//! member are sized differently — expansion already calls that an error.
//!
//! # The search
//!
//! For each slot, the candidates are the variants of the component's base name
//! — `A:4x16`, `A:5x16`, … for a component written `A:x`, and for an undecided
//! `A` the base is the whole name — filtered to those
//! that could go there at all: the box must fit the slot across the axis, a
//! `:WxH` in the name must be true, and **a name drawn for another direction is
//! not a candidate** (`compose::direction_rank` = 2, i.e. a `-r` variant for the
//! left slot of a `⿰`). The component as currently written is always a
//! candidate, whatever it says, since it is the source's own choice and not an
//! alternative being proposed — unless it is undecided, which names no drawing
//! that could fill anything.
//!
//! The score of a layout is how far its clearances fall outside the range,
//! summed — each of the n+1 clearances, plus their total, exactly the set of
//! numbers the check warns about. Zero is "no warning at all".
//!
//! # Why the gaps need no search
//!
//! Because the clearances *are* the free variables, and their sum is not one.
//! Write the layout as its clearances `c₀ … c_k` (k = n, the parts' count):
//! placing the parts is the same as choosing `c₀ … c_{k-1}` freely, because
//! moving a part along the axis moves exactly the two clearances beside it in
//! opposite directions. And their sum telescopes to
//!
//! ```text
//! T = near(first) + Σ facing(a, b) + (extent - 1 - far(last))
//! ```
//!
//! which mentions no position at all — it is a property of the *variants*, so
//! the last clearance is whatever the others leave. So the search over gaps is
//! the question "which integers summing to a fixed T are least far outside
//! `min..max`", which is arithmetic: if T fits in `(k+1) · min ..= (k+1) · max`
//! the answer is zero and every clearance can be in range; otherwise the least
//! possible is the shortfall itself, and it is reached exactly when every
//! clearance is on the range's near side. [`arrange`] is those three cases.
//!
//! Only the variants are searched, and the search is the product of the slots'
//! candidate lists, which is a handful.
//!
//! # Which of the equally good answers
//!
//! The score alone leaves many. In order:
//!
//! 1. **more variants that state a direction** — a `-l` name in the left slot
//!    says the drawing was made for that slot, and a source that says so is
//!    worth more than one that leaves it to be inferred;
//! 2. **the smallest sum of the two edge clearances** — the parts are pushed
//!    out against the glyph's box and the room they leave each other is what
//!    grows. This is the one that decides `⿰` between "0 1 0" and "0 0 1", and
//!    it is the whole of what makes the result look composed rather than
//!    shoved to one side;
//! 3. **the most even inner clearances**, when there are two of them (`⿲`/`⿳`);
//! 4. **lexicographically smallest**, left clearance first, so that what is
//!    left over lands at the near edge rather than anywhere;
//! 5. **the line as written**, then the names in order — so a run over an
//!    unchanged source is a no-op and the output is reproducible.
//!
//! Steps 2–4 are the same order [`arrange`] builds one layout in; they appear
//! again as a comparison because two *different* variant choices also have to
//! be ordered against each other.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::compose::{AxisFrontier, Direction, InkProfile, VariantSpec, facing_offset};
use crate::document::{ComposeItem, Document, DocumentItem, GlyphCompose, PixelGrid};

/// One IDC line the optimizer would rewrite.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearanceFix {
    /// The glyph whose block the line is in.
    pub glyph: String,
    /// Where that glyph was in the document the plan was made from — a hint,
    /// re-checked against the name when the fix is applied.
    pub item_idx: usize,
    /// Which IDC line of the block, in written order. Always 0 in a source
    /// that builds, a second line being an error.
    pub compose_idx: usize,
    /// The line as it stands, canonically formatted. For the report; what is
    /// on the line is whatever the author wrote.
    pub old_line: String,
    /// The line to put there instead.
    pub new_line: String,
    /// The score before and after: how far the clearances fall outside the
    /// range, summed. `after < before` always holds.
    ///
    /// `None` before means the line had no layout to score — a component had
    /// not picked its variant, so there was nothing measured rather than
    /// something measured badly.
    pub before: Option<i32>,
    pub after: i32,
}

/// What one document's fixes are, in the order the lines appear.
#[derive(Clone, Debug)]
pub struct DocumentFixes {
    /// Index into the `docs` slice the plan was made from.
    pub doc_idx: usize,
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    pub path: PathBuf,
    pub fixes: Vec<ClearanceFix>,
}

/// Candidates are capped per slot, and their product per line, so that a source
/// with an unexpectedly large variant family cannot turn one line into an hour.
/// A real one has a handful: the widest measured Han radical family is 5.
const MAX_CANDIDATES: usize = 32;
const MAX_COMBINATIONS: usize = 32_768;

/// Plan every IDC line rewrite the source's `audit ideal-clearance` rules ask
/// for. Reads the documents and nothing else; see [`crate::fix`] for who
/// applies the result.
pub fn optimize_clearance(docs: &[&Document]) -> Vec<DocumentFixes> {
    let rules = crate::audit::AuditRules::collect(docs).ideal_clearance;
    if rules.is_empty() {
        return Vec::new();
    }
    let inventory = Inventory::collect(docs);

    let mut out: Vec<DocumentFixes> = Vec::new();
    for (doc_idx, doc) in docs.iter().enumerate() {
        let mut fixes: Vec<ClearanceFix> = Vec::new();
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Glyph { name, body } = item else {
                continue;
            };
            // A glyph split twice is an error the source has to answer first;
            // both lines claim to be the whole shape, so neither is a layout
            // to improve.
            if body.compose.len() != 1 {
                continue;
            }
            let glyph = name.display();
            if !is_plain_name(&glyph) {
                continue;
            }
            let Some((_, min, max)) = rules.for_glyph(&glyph) else {
                continue;
            };
            let Some(parent) = body.declared_extent() else {
                continue;
            };
            for (compose_idx, compose) in body.compose.iter().enumerate() {
                let Some((new_line, before, after)) =
                    optimize_line(&inventory, parent, compose, min as i32, max as i32)
                else {
                    continue;
                };
                fixes.push(ClearanceFix {
                    glyph: glyph.clone(),
                    item_idx,
                    compose_idx,
                    old_line: compose.format_line(),
                    new_line,
                    before,
                    after,
                });
            }
        }
        if !fixes.is_empty() {
            out.push(DocumentFixes {
                doc_idx,
                path: doc.path.clone(),
                fixes,
            });
        }
    }
    out
}

/// A name the optimizer is willing to reason about: one glyph, spelled out.
///
/// A pattern names a family whose members are composed differently, and a `$`
/// is a name part that is not substituted here; neither is a thing a single
/// rewritten line could be right for.
fn is_plain_name(name: &str) -> bool {
    !crate::pattern::is_name_pattern(name) && !name.contains('$')
}

/// One part's ink, and the box [`InkProfile::of`] measures it over.
struct PartGrid<'a> {
    grid: &'a PixelGrid,
    scale: u8,
    origin: (i16, i16),
    extent: (u16, u16),
}

/// The glyphs a slot could be filled with, and what is known about each.
///
/// Names are canonicalized through the source's aliases exactly as the
/// expansion pass does, so a component written as an alias is sized and
/// measured by the glyph it actually is.
struct Inventory<'a> {
    /// Declared box per glyph name; `None` for a glyph whose header declares
    /// no `W H`, which no component may be.
    boxes: HashMap<String, Option<(u16, u16)>>,
    /// The grid of every glyph that draws itself entirely with its own pixels.
    /// A composite draws ink this pass cannot see, and half a part's ink
    /// measured is worse than none — the same rule `expand.rs::ink_profiles`
    /// applies.
    /// Name → the grid to measure and the box to measure it over.
    grids: HashMap<String, PartGrid<'a>>,
    /// Base name (everything before the first `:`) → its variants.
    variants: HashMap<String, Vec<String>>,
    aliases: crate::alias::AliasMap,
    /// Memoized [`InkProfile`]s: one part is a component of hundreds of glyphs,
    /// and its profile is the same in every one of them.
    profiles: std::cell::RefCell<HashMap<String, Option<Rc<InkProfile>>>>,
}

impl<'a> Inventory<'a> {
    fn collect(docs: &[&'a Document]) -> Self {
        let name_parts = crate::document::collect_name_parts(docs);
        let mut inv = Self {
            boxes: HashMap::new(),
            grids: HashMap::new(),
            variants: HashMap::new(),
            aliases: crate::alias::AliasMap::collect(docs, &name_parts),
            profiles: std::cell::RefCell::new(HashMap::new()),
        };
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Glyph { name, body } = item else {
                    continue;
                };
                let name = name.display();
                if !is_plain_name(&name) {
                    continue;
                }
                if inv.boxes.contains_key(&name) {
                    continue; // first definition wins, as everywhere else
                }
                if let (Some(pixels), true, true) = (
                    body.pixels.as_ref(),
                    body.refs.is_empty(),
                    body.compose.is_empty(),
                ) {
                    let extent = body.declared_extent().unwrap_or_else(|| {
                        let s = body.scale.max(1) as u16;
                        (pixels.width / s, pixels.height / s)
                    });
                    inv.grids.insert(
                        name.clone(),
                        PartGrid {
                            grid: pixels,
                            scale: body.scale,
                            origin: body.declared_origin(),
                            extent,
                        },
                    );
                }
                if let Some((base, _)) = name.split_once(':') {
                    inv.variants
                        .entry(base.to_string())
                        .or_default()
                        .push(name.clone());
                }
                inv.boxes.insert(name, body.declared_extent());
            }
        }
        for names in inv.variants.values_mut() {
            names.sort();
        }
        inv
    }

    fn canonical(&self, name: &str) -> String {
        let mut name = name.to_string();
        self.aliases.canonicalize(&mut name);
        name
    }

    /// The ink of a name, or `None` when it has none this pass can read.
    fn profile(&self, name: &str) -> Option<Rc<InkProfile>> {
        if let Some(cached) = self.profiles.borrow().get(name) {
            return cached.clone();
        }
        let computed = self
            .grids
            .get(name)
            .map(|g| Rc::new(InkProfile::of(g.grid, g.scale, g.origin, g.extent)));
        self.profiles
            .borrow_mut()
            .insert(name.to_string(), computed.clone());
        computed
    }

    /// One name, ready to be put in a slot — or `None` when it cannot go there.
    fn candidate(&self, written: &str, cross: u16, horizontal: bool) -> Option<Candidate> {
        let canonical = self.canonical(written);
        let (w, h) = (*self.boxes.get(&canonical)?)?;
        let (along, across) = if horizontal { (w, h) } else { (h, w) };
        if across != cross {
            return None;
        }
        // A name that lies about its own size is a source error, not a variant
        // to choose from.
        let spec = VariantSpec::parse(&canonical);
        if spec.size.is_some_and(|size| size != (w, h)) {
            return None;
        }
        let profile = self.profile(&canonical)?;
        Some(Candidate {
            frontier: profile.frontier(horizontal)?,
            name: written.to_string(),
            extent: along as i32,
            directed: spec.direction.is_some(),
            profile,
        })
    }

    /// Everything that could fill the slot `current` fills now, `current`
    /// itself first.
    ///
    /// A component that has not picked its variant yet is *not* itself a
    /// candidate — it names no drawing at all — and its whole name is the base
    /// whose family fills the slot instead. That is the one case where the list
    /// can come back without the name the line is written with.
    fn candidates(
        &self,
        current: &str,
        slot: Option<Direction>,
        cross: u16,
        horizontal: bool,
    ) -> Vec<Candidate> {
        let mut out = Vec::new();
        let canonical = self.canonical(current);
        let base = if crate::compose::is_undecided(&canonical) {
            canonical.clone()
        } else {
            let Some(mine) = self.candidate(current, cross, horizontal) else {
                return out;
            };
            out.push(mine);
            let Some((base, _)) = canonical.split_once(':') else {
                return out;
            };
            base.to_string()
        };
        for name in self.variants.get(&base).into_iter().flatten() {
            if out.len() >= MAX_CANDIDATES {
                break;
            }
            if *name == canonical || out.iter().any(|c| self.canonical(&c.name) == *name) {
                continue;
            }
            // A drawing made for the other side of the glyph is not an
            // alternative for this slot; see `compose::direction_rank`.
            if crate::compose::direction_rank(name, slot) > 1 {
                continue;
            }
            if let Some(candidate) = self.candidate(name, cross, horizontal) {
                out.push(candidate);
            }
        }
        out
    }
}

/// One name a slot could hold, with everything the score needs from it.
struct Candidate {
    name: String,
    /// The box's extent along the split axis, in declared units.
    extent: i32,
    frontier: AxisFrontier,
    /// Whether the name states an `l`/`r`/`u`/`d`/`c`.
    directed: bool,
    profile: Rc<InkProfile>,
}

/// How the optimizer orders two answers. Derived `Ord` is the whole rule: the
/// fields are in the order the module docs list them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    /// How far outside the range the layout is, summed. The objective.
    score: i32,
    /// More directed names first.
    directed: std::cmp::Reverse<usize>,
    /// The two edge clearances, added.
    edge_sum: i32,
    /// How far apart the two inner clearances are (`⿲`/`⿳` only; 0 otherwise).
    inner_spread: i32,
    clearances: Vec<i32>,
    /// `false` — the variants as written — sorts first.
    changed: bool,
    names: Vec<String>,
}

/// Plan one IDC line: `(the line to write, score before, score after)`, or
/// `None` when the line does not warn, cannot be measured, or cannot be
/// improved.
fn optimize_line(
    inv: &Inventory,
    parent: (u16, u16),
    compose: &GlyphCompose,
    lo: i32,
    hi: i32,
) -> Option<(String, Option<i32>, i32)> {
    let op = compose.op;
    let horizontal = op.horizontal();
    let (axis_extent, cross_extent) = match horizontal {
        true => (parent.0 as i32, parent.1),
        false => (parent.1 as i32, parent.0),
    };
    let written: Vec<&str> = compose.part_names().collect();
    if written.len() != op.arity() {
        return None;
    }
    if written.iter().any(|n| !is_plain_name(n)) {
        return None;
    }
    // An undecided component has not chosen a width, so the line has no layout
    // to measure — the check stands down and reports a TODO instead. There is
    // still something to do here, though: picking from the family it names is
    // exactly what the TODO asks for, so the line is optimized *towards* a
    // decision rather than away from a warning. Its `before` is `None`, and the
    // "must lower the score" rule below has nothing to compare against.
    let undecided = written.iter().any(|n| crate::compose::is_undecided(n));

    // As written: the parts where the line's own gaps put them.
    let before = match undecided {
        true => None,
        false => {
            let current: Vec<Candidate> = written
                .iter()
                .map(|name| inv.candidate(name, cross_extent, horizontal))
                .collect::<Option<Vec<_>>>()?;
            let placed = walk(compose, &current);
            let as_written: Vec<&Candidate> = current.iter().collect();
            let before = score(
                &clearances_at(&as_written, &placed, axis_extent, horizontal)?,
                lo,
                hi,
            );
            if before == 0 {
                return None; // nothing warns, so nothing to fix
            }
            Some(before)
        }
    };

    let slots: Vec<Vec<Candidate>> = written
        .iter()
        .enumerate()
        .map(|(slot, name)| inv.candidates(name, op.slot_direction(slot), cross_extent, horizontal))
        .collect();
    let combinations: usize = slots.iter().map(Vec::len).product();
    if combinations == 0 || combinations > MAX_COMBINATIONS {
        return None;
    }

    let mut best: Option<(Key, Vec<usize>)> = None;
    for pick in Combinations::new(&slots) {
        let chosen: Vec<&Candidate> = pick
            .iter()
            .enumerate()
            .map(|(s, &i)| &slots[s][i])
            .collect();
        let Some(key) = evaluate(&chosen, &written, axis_extent, horizontal, lo, hi) else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| key < *b) {
            best = Some((key, pick));
        }
    }
    let (key, pick) = best?;
    if before.is_some_and(|before| key.score >= before) {
        return None; // a line nobody can improve keeps its warning
    }
    let chosen: Vec<&Candidate> = pick
        .iter()
        .enumerate()
        .map(|(s, &i)| &slots[s][i])
        .collect();

    // Where the chosen clearances put each part. `c₀` is the first part's ink
    // against the near edge, and every later one is the distance from the
    // previous part's origin, which is what `facing_offset` is measured from.
    let mut positions = Vec::with_capacity(chosen.len());
    let mut at = key.clearances[0] - chosen[0].frontier.near;
    positions.push(at);
    for (i, pair) in chosen.windows(2).enumerate() {
        let facing = facing_offset(&pair[0].profile, &pair[1].profile, horizontal)?;
        at += key.clearances[i + 1] - facing;
        positions.push(at);
    }

    let line = write_line(compose, &chosen, &positions)?;
    // The arithmetic says what this layout measures; the measurement says so
    // too, or the line is left alone. Cheap, and the alternative is a command
    // that quietly writes a layout it was wrong about.
    let verified = clearances_at(&chosen, &positions, axis_extent, horizontal)?;
    if score(&verified, lo, hi) != key.score {
        return None;
    }
    Some((line, before, key.score))
}

/// Score one candidate combination, and the layout it is scored at.
fn evaluate(
    chosen: &[&Candidate],
    written: &[&str],
    axis_extent: i32,
    horizontal: bool,
    lo: i32,
    hi: i32,
) -> Option<Key> {
    let n = chosen.len() + 1;
    // The sum every layout of these variants has, whatever the gaps do.
    let mut total = chosen[0].frontier.near + (axis_extent - 1 - chosen[n - 2].frontier.far);
    for pair in chosen.windows(2) {
        total += facing_offset(&pair[0].profile, &pair[1].profile, horizontal)?;
    }
    let clearances = arrange(n, total, lo, hi);
    Some(Key {
        score: score(&clearances, lo, hi),
        directed: std::cmp::Reverse(chosen.iter().filter(|c| c.directed).count()),
        edge_sum: clearances[0] + clearances[n - 1],
        inner_spread: match n {
            4 => (clearances[1] - clearances[2]).abs(),
            _ => 0,
        },
        clearances,
        changed: chosen.iter().zip(written).any(|(c, w)| c.name != *w),
        names: chosen.iter().map(|c| c.name.clone()).collect(),
    })
}

/// Where the line's own gaps put each part, in declared units.
fn walk(compose: &GlyphCompose, parts: &[Candidate]) -> Vec<i32> {
    let mut at = 0i32;
    let mut out = Vec::with_capacity(parts.len());
    for item in &compose.items {
        match item {
            ComposeItem::Gap(gap) => at += *gap as i32,
            ComposeItem::Part { .. } => {
                out.push(at);
                at += parts[out.len() - 1].extent;
            }
        }
    }
    out
}

/// The n+1 clearances of parts placed at `positions`. `None` when two
/// neighbours share no line on which both draw.
fn clearances_at(
    parts: &[&Candidate],
    positions: &[i32],
    axis_extent: i32,
    horizontal: bool,
) -> Option<Vec<i32>> {
    let last = parts.len() - 1;
    let mut out = vec![positions[0] + parts[0].frontier.near];
    for i in 0..last {
        let facing = facing_offset(&parts[i].profile, &parts[i + 1].profile, horizontal)?;
        out.push(positions[i + 1] - positions[i] + facing);
    }
    out.push(axis_extent - 1 - (positions[last] + parts[last].frontier.far));
    Some(out)
}

/// The line that places `chosen` at `positions`: the same operator and comment,
/// the chosen names, and the gaps the positions imply.
///
/// A gap of zero is not written, and neither is a trailing one — it would move
/// nothing, the cursor having nothing left to place.
fn write_line(compose: &GlyphCompose, chosen: &[&Candidate], positions: &[i32]) -> Option<String> {
    let mut items: Vec<ComposeItem> = Vec::new();
    let mut cursor = 0i32;
    for (i, part) in chosen.iter().enumerate() {
        let gap = positions[i] - cursor;
        if gap != 0 {
            items.push(ComposeItem::Gap(i16::try_from(gap).ok()?));
        }
        // A component that did not change keeps how it was written, `@` form
        // and all; a new one is written out as the glyph it names.
        let raw_name = compose.items.iter().find_map(|item| match item {
            ComposeItem::Part { name, raw_name } if *name == part.name => raw_name.clone(),
            _ => None,
        });
        items.push(ComposeItem::Part {
            name: part.name.clone(),
            raw_name,
        });
        cursor = positions[i] + part.extent;
    }
    Some(
        GlyphCompose {
            op: compose.op,
            items,
            // A rewrite picks variants and moves gaps; whether the line is
            // conditional is not its business.
            if_exists: compose.if_exists,
            comment: compose.comment.clone(),
        }
        .format_line(),
    )
}

/// How far `v` is from the inclusive range; 0 inside it.
fn distance(v: i32, lo: i32, hi: i32) -> i32 {
    (lo - v).max(v - hi).max(0)
}

/// What the check would warn about, as one number: every clearance's distance
/// from the range, plus their total's — the total being held to the range too
/// (see [`crate::compose`] for why).
fn score(clearances: &[i32], lo: i32, hi: i32) -> i32 {
    let total: i32 = clearances.iter().sum();
    clearances.iter().map(|c| distance(*c, lo, hi)).sum::<i32>() + distance(total, lo, hi)
}

/// The `n` clearances summing to `total` that the module's rules pick: as far
/// inside `lo..=hi` as the total allows, then out at the edges, then even in
/// the middle, then lexicographically least.
///
/// The feasible set is a box every clearance shares, because the least possible
/// cost pins them all to one side of the range:
///
/// - `total` inside `n·lo ..= n·hi` — every clearance can be in `lo..=hi`, cost 0;
/// - `total` below it — the cost is the shortfall, reached exactly when no
///   clearance exceeds `lo`;
/// - `total` above it — the cost is the excess, reached exactly when none is
///   below `hi`.
///
/// Inside that box the rest is a greedy walk: fill the inner clearances as far
/// as the box and the edges' own room allow (which is what minimizes the edge
/// pair, their sum being `total` less the inner ones), split the inner sum as
/// evenly as it goes with the smaller share first, then give the near edge the
/// least it may take.
fn arrange(n: usize, total: i32, lo: i32, hi: i32) -> Vec<i32> {
    debug_assert!(n >= 3, "an IDC line has at least two parts");
    let count = n as i32;
    let (low, high) = if total < count * lo {
        (None, Some(lo))
    } else if total > count * hi {
        (Some(hi), None)
    } else {
        (Some(lo), Some(hi))
    };
    let inner = n - 2;
    // Only one of the two bounds can be missing, so a bound always survives.
    let inner_sum = [high.map(|u| u * inner as i32), low.map(|l| total - 2 * l)]
        .into_iter()
        .flatten()
        .min()
        .expect("one side of the box is always bounded");
    let edge_sum = total - inner_sum;
    let near = [low, high.map(|u| edge_sum - u)]
        .into_iter()
        .flatten()
        .max()
        .expect("one side of the box is always bounded");

    let share = inner_sum.div_euclid(inner as i32);
    let over = (inner_sum - share * inner as i32) as usize;
    let mut out = Vec::with_capacity(n);
    out.push(near);
    out.extend(std::iter::repeat_n(share, inner - over));
    out.extend(std::iter::repeat_n(share + 1, over));
    out.push(edge_sum - near);
    out
}

/// The cartesian product of the slots' candidate lists, as index vectors. The
/// last slot varies fastest, so the order is the one the lists are written in.
struct Combinations {
    lengths: Vec<usize>,
    next: Option<Vec<usize>>,
}

impl Combinations {
    fn new(slots: &[Vec<Candidate>]) -> Self {
        let lengths: Vec<usize> = slots.iter().map(Vec::len).collect();
        let next = lengths
            .iter()
            .all(|n| *n > 0)
            .then(|| vec![0usize; lengths.len()]);
        Self { lengths, next }
    }
}

impl Iterator for Combinations {
    type Item = Vec<usize>;

    fn next(&mut self) -> Option<Vec<usize>> {
        let current = self.next.take()?;
        let mut advanced = current.clone();
        for slot in (0..advanced.len()).rev() {
            advanced[slot] += 1;
            if advanced[slot] < self.lengths[slot] {
                self.next = Some(advanced);
                return Some(current);
            }
            advanced[slot] = 0;
        }
        Some(current)
    }
}

#[cfg(test)]
#[path = "clearance_tests.rs"]
mod tests;
