//! Shortest input sequences: what has to be typed to put a glyph on the screen
//! when no `map` names it.
//!
//! A `remap` target is reached by a *sequence*, not by a code point — the flag
//! glyphs are two regional indicators, `flag-gbeng` is a black flag and six
//! tags — so a specimen has nothing to key a cell on. This module answers, for
//! a set of target glyph names, the shortest code point sequence that produces
//! each. What is done with the answer is the caller's business; nothing here
//! renders anything.
//!
//! # The unit of the search is a string, not a glyph
//!
//! The obvious formulation is a cost per glyph — `cost(target)` is the cheapest
//! `Σ cost(source) + Σ cost(context)` over the rules that produce it, solved as
//! a forward sweep over the groups in lookup order. It is very cheap and it is
//! **wrong**, because two glyphs standing next to each other are written in the
//! *same* string: `regional-indicator-a-left` is derived with a following
//! regional indicator as its lookahead, and that lookahead is the very glyph
//! `regional-indicator-b-right` is derived *from*. Summing the two derivations
//! writes that shared position twice, which for `flag-ab` yields five code
//! points where two are wanted — and five that render two other flags and a
//! leftover indicator, so the answer is not merely long but false.
//!
//! No per-glyph cost can express that, so this module does not try. It runs the
//! cascade instead ([`Cascade::shape`]): cmap, then one left-to-right pass per
//! remap group, first matching rule wins, lookbehind against what the pass has
//! already rewritten and lookahead against what it has not. That is the same
//! model [`crate::render::ttf_builder::gsub`] builds the lookups under, and it
//! makes every sequence this module reports one that has been *checked* rather
//! than derived. There is nothing left for a shaper to confirm, which is why no
//! font is built here.
//!
//! # How an answer is found
//!
//! The per-glyph sweep survives as [`Cascade::sweep`], demoted to a *candidate*
//! generator, because it is exact whenever a rule's context is typed rather than
//! derived — which is most rules, `62` of the `107` in `font/` having no context
//! at all, and the Hangul jamo rules taking theirs straight from the cmap.
//! [`Cascade::solve`] then:
//!
//! 1. shapes each candidate and keeps it if the target actually appears — one
//!    shaping settles every target in the run, not just the one asked for;
//! 2. for what is left, searches the alphabet the failed candidate names. A
//!    candidate fails by writing a shared part of the string twice, so it names
//!    the right code points in the wrong number: its own distinct code points
//!    are a small, well-aimed alphabet. `flag-ab`'s candidate is `AABBB`-shaped
//!    over `{🇦, 🇧}`, and the four two-letter words over that alphabet contain
//!    the answer.
//!
//! The search is bounded by [`REPAIR_BUDGET`] shapings and gives up rather than
//! guess, so a target with no answer is simply absent from the result.
//!
//! Over `font/` that answers all `1531` of them: the flags as their two regional
//! indicators, `flag-gbeng` as a black flag and six tags, and every composed
//! jamo as the two or three it is written with.
//!
//! # Cost
//!
//! The sweep is one look at each expanded rule per group, `O(Σ|rule|)`, with the
//! context resolved once per *line* rather than once per expansion — which is
//! what keeps a Hangul rule's 137-name coverage set additive instead of
//! multiplicative. Shaping is `O(|sequence| × rules in a group)`, run once per
//! target plus whatever the repair search costs. Both read the expansion
//! `collect_gsub_data` has already produced for the GSUB tables, so no name is
//! expanded twice — over `font/` the expansion is 731 ms and everything here is
//! 146 ms of it, nearly all in the searches the 53 stubborn targets need.
//!
//! # What is not modelled
//!
//! Whether a group's feature is one a shaper turns on by default, and whether
//! script segmentation would split the sequence before the rules see it. The
//! caller chooses which groups to hand over; every `feature` in `font/` is
//! default-on (`ccmp`, `liga`, `calt`, `locl`, `ljmo`, `vjmo`, `tjmo`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// How many times a group's rules are re-read before its own output stops
/// improving.
///
/// A lookbehind matches what the pass has already rewritten, so a rule may
/// depend on a rule of its *own* group — `flag-ri` is written exactly that way,
/// its first rule reading the `-left` its second one produces. One reading in
/// declaration order therefore misses it and a second catches it. The loop stops
/// as soon as a round changes nothing; the cap is only there so that a source
/// that chains further than anyone intended cannot spin, the way
/// [`crate::exists`] bounds its own fixpoint.
const SWEEP_ROUNDS: usize = 8;

/// How many sequences the repair search may shape, per alphabet, before giving
/// up on a target.
///
/// Nearly every target is answered by its own candidate and never reaches the
/// search at all; over `font/` it is `53` of `1531` that do, and what they cost
/// is most of the 141 ms the whole cascade takes. The budget is what stops a
/// long candidate over a wide alphabet from turning that into minutes, and a
/// target that exhausts it is reported as having no sequence rather than a
/// guessed one — `tok-tally-6`, eight code points over three letters, is the
/// one in `font/` that needs most of it.
const REPAIR_BUDGET: usize = 32768;

/// One expanded `remap` line: the context every expansion of the line shares,
/// and one input/output glyph sequence per expansion.
///
/// This is `ttf_builder::ExpandedRemap` seen by borrow, so that a caller who
/// already has the GSUB expansion pays nothing to ask this question.
#[derive(Clone, Copy, Debug)]
pub struct RemapLine<'a> {
    /// One coverage set per position, in reading order: the last position is the
    /// one immediately before the input. (`gsub.rs` reverses this into
    /// OpenType's nearest-first backtrack order; nothing else does.)
    pub lookbehind: &'a [Vec<String>],
    /// One input glyph sequence per expansion, parallel to `target`. A sequence
    /// longer than one glyph is a ligature.
    pub source: &'a [Vec<String>],
    /// One output glyph sequence per expansion, parallel to `source`.
    pub target: &'a [Vec<String>],
    /// One coverage set per position, the first immediately after the input.
    pub lookahead: &'a [Vec<String>],
}

/// The cmap and the remap groups of one face, in lookup order.
pub struct Cascade<'a> {
    cmap: HashMap<u32, &'a str>,
    uvs: HashMap<(u32, u32), &'a str>,
    selectors: HashSet<u32>,
    /// The cheapest way to type each glyph a `map` names directly, which is
    /// where every derivation bottoms out.
    seed: BTreeMap<&'a str, Vec<u32>>,
    groups: &'a [Vec<RemapLine<'a>>],
}

impl<'a> Cascade<'a> {
    /// `cmap` is (code point, glyph); `uvs` is (base, selector, glyph); `groups`
    /// are the remap groups **in lookup order** — see
    /// [`crate::document::remap_group_order`], which is the only thing that
    /// knows it.
    pub fn new(
        cmap: &'a [(u32, String)],
        uvs: &'a [(u32, u32, String)],
        groups: &'a [Vec<RemapLine<'a>>],
    ) -> Self {
        let mut this = Cascade {
            cmap: HashMap::new(),
            uvs: HashMap::new(),
            selectors: HashSet::new(),
            seed: BTreeMap::new(),
            groups,
        };
        for (cp, name) in cmap {
            if name.is_empty() {
                continue;
            }
            this.cmap.entry(*cp).or_insert(name.as_str());
            offer(&mut this.seed, name.as_str(), vec![*cp]);
        }
        for (base, sel, name) in uvs {
            if name.is_empty() {
                continue;
            }
            this.uvs.insert((*base, *sel), name.as_str());
            this.selectors.insert(*sel);
            offer(&mut this.seed, name.as_str(), vec![*base, *sel]);
        }
        this
    }

    /// The glyph run a code point sequence produces: cmap, then one pass per
    /// group.
    ///
    /// `None` when some code point has no cmap entry — a sequence that cannot
    /// even be spelled is not worth shaping.
    pub fn shape(&self, cps: &[u32]) -> Option<Vec<&'a str>> {
        let mut run: Vec<&'a str> = Vec::with_capacity(cps.len());
        let mut i = 0;
        while i < cps.len() {
            if i + 1 < cps.len()
                && self.selectors.contains(&cps[i + 1])
                && let Some(glyph) = self.uvs.get(&(cps[i], cps[i + 1]))
            {
                run.push(glyph);
                i += 2;
                continue;
            }
            run.push(self.cmap.get(&cps[i]).copied()?);
            i += 1;
        }
        for group in self.groups {
            run = apply_group(group, &run);
        }
        Some(run)
    }

    /// The shortest code point sequence that puts each of `targets` on the
    /// screen. A target with no answer is absent.
    pub fn solve(&self, targets: &BTreeSet<&str>) -> BTreeMap<&'a str, Vec<u32>> {
        let mut found: BTreeMap<&'a str, Vec<u32>> = BTreeMap::new();
        let candidates = self.sweep();

        // Checking one candidate settles every target its run contains, so the
        // targets a later candidate would have answered are often already done.
        for (name, cps) in &candidates {
            if targets.contains(*name) && !found.contains_key(*name) {
                self.record(&mut found, cps, targets);
            }
        }

        // What is left is a target whose candidate wrote a shared stretch of the
        // string more than once.
        for name in targets {
            if found.contains_key(name) {
                continue;
            }
            let Some(candidate) = candidates.get(name) else {
                continue;
            };
            self.repair(&candidate.clone(), name, &candidates, targets, &mut found);
        }
        found
    }

    /// Shape one sequence and credit it to every target it turns out to produce.
    fn record(
        &self,
        found: &mut BTreeMap<&'a str, Vec<u32>>,
        cps: &[u32],
        targets: &BTreeSet<&str>,
    ) {
        let Some(run) = self.shape(cps) else {
            return;
        };
        for glyph in run {
            if targets.contains(glyph) {
                offer(found, glyph, cps.to_vec());
            }
        }
    }

    /// Search for a sequence that leaves `stop` standing, shortest first.
    ///
    /// Two alphabets, in order. The candidate's own distinct code points come
    /// first: it is wrong only by repetition, so it names the right code points
    /// in the wrong number, and the answer is usually a word or two into the
    /// enumeration.
    ///
    /// That is too narrow when the glyph is one a *later* group consumes. Every
    /// two-indicator word over `{🇦, 🇲}` is a flag, so `regional-indicator-m-left`
    /// never survives to the end of the cascade and its own candidate can never
    /// show it; what is needed is a following indicator that makes *no* flag,
    /// which is a code point the candidate never mentions. The second alphabet
    /// is therefore everything the producing rules can spell — their inputs and
    /// every alternative of their context.
    fn repair(
        &self,
        candidate: &[u32],
        stop: &str,
        spellings: &BTreeMap<&'a str, Vec<u32>>,
        targets: &BTreeSet<&str>,
        found: &mut BTreeMap<&'a str, Vec<u32>>,
    ) {
        let narrow: Vec<Vec<u32>> = {
            let mut cps = candidate.to_vec();
            cps.sort_unstable();
            cps.dedup();
            cps.into_iter().map(|cp| vec![cp]).collect()
        };
        let wide = self.rule_alphabet(stop, spellings);
        for alphabet in [narrow, wide] {
            if self.enumerate(&alphabet, candidate.len(), stop, targets, found) {
                return;
            }
        }
    }

    /// Every word over `alphabet` up to `longest` units, shortest first, until
    /// `stop` is answered. Says whether it was.
    fn enumerate(
        &self,
        alphabet: &[Vec<u32>],
        longest: usize,
        stop: &str,
        targets: &BTreeSet<&str>,
        found: &mut BTreeMap<&'a str, Vec<u32>>,
    ) -> bool {
        if alphabet.is_empty() {
            return false;
        }
        let mut budget = REPAIR_BUDGET;
        for len in 1..=longest {
            // A length is enumerated whole or not at all: stopping part way
            // through one would make the answer depend on where the budget
            // happened to run out.
            let Some(count) = alphabet.len().checked_pow(len as u32) else {
                return false;
            };
            if count > budget {
                return false;
            }
            budget -= count;
            for word in words(alphabet, len) {
                self.record(found, &word, targets);
                if found.contains_key(stop) {
                    return true;
                }
            }
        }
        false
    }

    /// What the rules producing `target` can spell: their input glyphs and every
    /// alternative of every context position.
    fn rule_alphabet(
        &self,
        target: &str,
        spellings: &BTreeMap<&'a str, Vec<u32>>,
    ) -> Vec<Vec<u32>> {
        let mut out: BTreeSet<Vec<u32>> = BTreeSet::new();
        for group in self.groups {
            for line in group {
                let produces = line
                    .target
                    .iter()
                    .flatten()
                    .any(|name| name.as_str() == target);
                if !produces {
                    continue;
                }
                let context = line.lookbehind.iter().chain(line.lookahead);
                for name in line.source.iter().chain(context).flatten() {
                    if let Some(cps) = spellings.get(name.as_str()) {
                        out.insert(cps.clone());
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// A first guess at how to type every glyph, ignoring that neighbouring
    /// derivations share the string they are written in.
    ///
    /// One pass per group in lookup order, so a rule reads only what the groups
    /// before it produced — one lookup is one pass, and no subtable sees
    /// another's output at the same position. The one exception is the
    /// lookbehind, which matches glyphs the pass has already rewritten and so
    /// may read this group's own pending output; that is what [`SWEEP_ROUNDS`]
    /// iterates for.
    ///
    /// A group's `reversed` flag needs no handling here: it turns the pass
    /// around, and this approximation is symmetric in the two context sides.
    fn sweep(&self) -> BTreeMap<&'a str, Vec<u32>> {
        let mut best = self.seed.clone();
        for group in self.groups {
            let mut pending: BTreeMap<&'a str, Vec<u32>> = BTreeMap::new();
            for _ in 0..SWEEP_ROUNDS {
                let mut changed = false;
                for line in group {
                    // Context is resolved once per line, not once per
                    // expansion: a coverage set is shared by every expansion of
                    // the line it was written on, and some of them are large.
                    let Some(before) = context(line.lookbehind, &[&pending, &best]) else {
                        continue;
                    };
                    let Some(after) = context(line.lookahead, &[&best]) else {
                        continue;
                    };
                    for (source, target) in line.source.iter().zip(line.target) {
                        let Some(middle) = spell(source, &best) else {
                            continue;
                        };
                        let mut cps = before.clone();
                        cps.extend_from_slice(&middle);
                        cps.extend_from_slice(&after);
                        for name in target {
                            if name.is_empty() {
                                continue;
                            }
                            changed |= offer(&mut pending, name, cps.clone());
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            for (name, cps) in pending {
                offer(&mut best, name, cps);
            }
        }
        best
    }
}

/// One left-to-right pass of one group.
fn apply_group<'a>(group: &[RemapLine<'a>], run: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&'a str> = Vec::with_capacity(run.len());
    let mut at = 0;
    while at < run.len() {
        match first_match(group, &out, run, at) {
            // The pass advances past the input the rule consumed, so no later
            // rule of this group sees what it produced at this position.
            Some((consumed, produced)) => {
                out.extend(produced);
                at += consumed;
            }
            None => {
                out.push(run[at]);
                at += 1;
            }
        }
    }
    out
}

/// The first rule of the group that matches at `at`: how much input it consumes
/// and what it produces.
///
/// `done` is the rewritten prefix and `run` the untouched sequence, which is the
/// whole of the lookbehind/lookahead asymmetry: the pass has passed the former
/// and not the latter.
fn first_match<'a>(
    group: &[RemapLine<'a>],
    done: &[&'a str],
    run: &[&'a str],
    at: usize,
) -> Option<(usize, Vec<&'a str>)> {
    for line in group {
        if line.lookbehind.len() > done.len() {
            continue;
        }
        if !covers(line.lookbehind, &done[done.len() - line.lookbehind.len()..]) {
            continue;
        }
        for (source, target) in line.source.iter().zip(line.target) {
            if source.is_empty() || source.len() > run.len() - at {
                continue;
            }
            if !source
                .iter()
                .zip(&run[at..])
                .all(|(name, glyph)| name == glyph)
            {
                continue;
            }
            if !covers(line.lookahead, &run[at + source.len()..]) {
                continue;
            }
            return Some((source.len(), target.iter().map(String::as_str).collect()));
        }
    }
    None
}

/// Whether every coverage position matches the glyph aligned with it. A position
/// matches if *any* of its names does.
fn covers(positions: &[Vec<String>], glyphs: &[&str]) -> bool {
    positions.len() <= glyphs.len()
        && positions
            .iter()
            .zip(glyphs)
            .all(|(names, glyph)| names.iter().any(|name| name == glyph))
}

/// The cheapest known glyph of every coverage position, spelled out end to end.
///
/// `None` if some position has no known glyph at all: a rule whose context
/// cannot be written cannot fire, and that is one test for the whole line rather
/// than one per expansion.
fn context(positions: &[Vec<String>], maps: &[&BTreeMap<&str, Vec<u32>>]) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for names in positions {
        let cheapest = names
            .iter()
            .filter(|name| !name.is_empty())
            .filter_map(|name| {
                maps.iter()
                    .filter_map(|map| map.get(name.as_str()))
                    .min_by(|a, b| by_cost(a, b))
            })
            .min_by(|a, b| by_cost(a, b))?;
        out.extend_from_slice(cheapest);
    }
    Some(out)
}

/// The known spelling of every glyph of a sequence, end to end, or `None` if any
/// of them is unknown.
fn spell(names: &[String], map: &BTreeMap<&str, Vec<u32>>) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for name in names {
        if name.is_empty() {
            return None;
        }
        out.extend_from_slice(map.get(name.as_str())?);
    }
    Some(out)
}

/// Record `cps` for `name` unless something at least as good is already there,
/// and say whether that changed anything.
///
/// A tie keeps what is there: two sequences of one length are equally good to a
/// reader, and keeping the first makes the answer independent of the order the
/// documents were read in.
fn offer<'k>(map: &mut BTreeMap<&'k str, Vec<u32>>, name: &'k str, cps: Vec<u32>) -> bool {
    match map.get(name) {
        Some(old) if by_cost(&cps, old).is_ge() => false,
        _ => {
            map.insert(name, cps);
            true
        }
    }
}

/// Shorter first, then by code point, so that two equally short answers are
/// still ordered and the result is reproducible.
fn by_cost(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Every word of `len` units over `alphabet`, in lexicographic order. A unit is
/// a whole spelling, so a variation sequence stays one letter of the alphabet.
fn words(alphabet: &[Vec<u32>], len: usize) -> impl Iterator<Item = Vec<u32>> + '_ {
    let mut odometer = vec![0usize; len];
    let mut done = len == 0 || alphabet.is_empty();
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let word: Vec<u32> = odometer
            .iter()
            .flat_map(|&i| alphabet[i].iter().copied())
            .collect();
        let mut pos = len;
        loop {
            if pos == 0 {
                done = true;
                break;
            }
            pos -= 1;
            odometer[pos] += 1;
            if odometer[pos] < alphabet.len() {
                break;
            }
            odometer[pos] = 0;
        }
        Some(word)
    })
}

#[cfg(test)]
#[path = "reach_tests.rs"]
mod reach_tests;
