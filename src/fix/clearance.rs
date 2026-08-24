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
//! There are three kinds of report it acts on, and they are not the same act:
//!
//! - a **clearance warning**, where the line has a layout and the layout is
//!   outside the range. The search moves it inside, and the rewrite has to
//!   lower the score or it is not made;
//! - a **wrong-slot warning**, where a component is drawn for one side of the
//!   glyph and sits on another (`compose`'s "drawn for `-l` but sits in the
//!   `-r` slot"). Nothing about the *clearances* is wrong there, so a score is
//!   silent about it and the count of such components is an objective of its
//!   own — the second one, behind the score, so that no name is ever put right
//!   by making the layout worse;
//! - a **TODO**, where a component has not picked its variant
//!   ([`crate::compose::is_undecided`]). There is no layout at all then, so
//!   there is no score to lower — but the family the component names is on hand
//!   and choosing from it is exactly what the TODO asks for. Such a line is
//!   planned whatever it scores, since any decided layout is more than none,
//!   and its [`ClearanceFix::before`] is `None` to say so.
//!
//! What is skipped is what cannot be measured *after* a choice either: a
//! component that names nothing, a part with no ink of its own, an undecided
//! component whose family is empty.
//!
//! # A line that stands for a family
//!
//! A glyph block whose name is a pattern writes one line for every glyph it
//! declares, and each of those glyphs is composed of its own parts, sized on
//! their own. So what a rewrite may move there is what the family *shares*:
//! the gaps, and a component's variant **label** whenever the block's own
//! pattern does not reach it. A component written `han-4ee4-($han-regions):9x16`
//! says the same `9x16` for every glyph the block declares, so that label is
//! the family's answer and not one glyph's, and each glyph's own family is
//! asked for the same label in turn ([`slot_choices`]); a component written
//! `han-4ee4-g:(7|9)x16` says something different per glyph and is left alone.
//! The *base* is never searched — a name is one glyph's answer and cannot be
//! the family's.
//!
//! One set of gaps and labels then has to serve every glyph, which makes the
//! objective a different one: **the fewest glyphs warning at all**, and only then the
//! summed score and the same tie-breaks below. The warnings are a work queue
//! and its length is what the command is there to shorten, so a family in
//! which one more glyph is finished beats one in which every glyph is a little
//! less wrong — even when that trade costs the sum. [`optimize_pattern_line`]
//! is the whole of it, and [`Member`] is why it stays cheap over a family of
//! thousands: one glyph costs a handful of additions per set of gaps, whatever
//! the labels chose.
//!
//! # The search
//!
//! For each slot, the candidates are the variants of the component's base name
//! — `A:4x16`, `A:5x16`, … for a component written `A:x`, and for an undecided
//! `A` the base is the whole name — filtered to those
//! that could go there at all: the box must fit the slot across the axis, a
//! `:WxH` in the name must be true, **a drawing as long as the glyph's own
//! axis is not a candidate** — it fills the glyph on its own, so nothing else
//! on the line has anywhere to stand ([`fits_beside`]) — and **a name drawn for
//! another direction is not a candidate**
//! (`compose::direction_rank` = 2, i.e. a `-r` variant for the
//! left slot of a `⿰`). The component as currently written is always a
//! candidate, whatever it says, since it is the source's own choice and not an
//! alternative being proposed — unless it is undecided, which names no drawing
//! that could fill anything.
//!
//! The score of a layout is how far its clearances fall outside the range,
//! summed — each of the n+1 clearances, plus their total, exactly the set of
//! numbers the check warns about. Zero is "no clearance warning at all", which
//! is not the same as no warning: a wrong-slot component warns at any score,
//! and is counted beside it.
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
//! `audit max-contact-run` does not disturb any of that, because it is not a
//! second kind of number: what a junction owes it is a property of the pair and
//! not of where the line puts them, so it lands inside the facing measurement
//! ([`crate::compose::effective_facing`]) exactly where a hardblank would have.
//! Every sum, arrangement and score below reads it without knowing it is
//! there.
//!
//! # Which of the equally good answers
//!
//! The score alone leaves many. In order:
//!
//! 1. **fewer components in a slot they are not drawn for** — this is the
//!    warning above, so it comes before every tie-break; then **more variants
//!    that state their slot's own direction** — a `-l` name in the left slot
//!    says the drawing was made for that slot, and a source that says so is
//!    worth more than one that leaves it to be inferred. They are two numbers
//!    and not one sum of ranks, because a sum cannot tell a `⿰` whose slots
//!    hold `[-l, -r]` reversed — one perfect name and one wrong one — from one
//!    holding two unmarked names, and only the first of those warns;
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

use crate::compose::{AxisFrontier, Direction, InkProfile, VariantSpec, effective_facing};
use crate::document::{ComposeItem, Document, DocumentItem, GlyphBody, GlyphCompose, PixelGrid};

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
    /// range, summed — over the whole family for a pattern line.
    ///
    /// `after < before` holds for a line that stands for one glyph. For a
    /// pattern line the objective is [`glyphs_warning`](Self::glyphs_warning)
    /// first, so `after` may be the larger of the two when the rewrite
    /// finishes a glyph at the others' expense.
    ///
    /// `None` before means the line had no layout to score — a component had
    /// not picked its variant, so there was nothing measured rather than
    /// something measured badly.
    pub before: Option<i32>,
    pub after: i32,
    /// How many components sit in a slot their name is not drawn for, before
    /// and after — the other thing this command answers, and the one a score of
    /// zero says nothing about. For a pattern line it counts *slots*, the
    /// label a slot carries being one thing the whole family shares. `None`
    /// when the line was not scored as written at all (an undecided
    /// component), there being no before to compare against.
    pub mismatched: Option<(usize, usize)>,
    /// How many of the glyphs the line stands for warn at all, before and
    /// after. `None` for a line that stands for one glyph, which either warns
    /// or is not planned.
    pub glyphs_warning: Option<(usize, usize)>,
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
/// The same, for a pattern line, where the unit of work is one glyph of the
/// family scored at one set of gaps: a few million of those is a few
/// milliseconds, and no real line comes close.
const MAX_PATTERN_WORK: usize = 4_194_304;

/// Plan every IDC line rewrite the source's `audit ideal-clearance` rules ask
/// for. Reads the documents and nothing else; see [`crate::fix`] for who
/// applies the result.
pub fn optimize_clearance(docs: &[&Document]) -> Vec<DocumentFixes> {
    let audit = crate::audit::AuditRules::collect(docs);
    let rules = &audit.ideal_clearance;
    if rules.is_empty() {
        return Vec::new();
    }
    // The base bindings, not a slice's: a `$` in an IDC line stands for a name
    // whichever face is being built, and a slice-scoped binding would make one
    // line several, which is not something one rewrite could be right for.
    let name_parts = crate::document::collect_name_parts(docs);
    let inventory = Inventory::collect(docs, &name_parts);

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
            let Some(parent) = body.declared_extent() else {
                continue;
            };
            for (compose_idx, compose) in body.compose.iter().enumerate() {
                // A pattern block is one line over a family, and what its
                // glyphs share is the gaps; a plain one is a layout of its own.
                let planned: Option<PlannedLine> = match is_plain_name(&glyph) {
                    true => rules.for_glyph(&glyph).and_then(|(_, min, max)| {
                        optimize_line(
                            &inventory,
                            parent,
                            compose,
                            min as i32,
                            max as i32,
                            audit.max_contact_run.for_glyph(&glyph).map(|(_, m)| m),
                        )
                    }),
                    false => optimize_pattern_line(
                        &inventory,
                        &audit,
                        &name_parts,
                        &glyph,
                        body.scale,
                        parent,
                        compose,
                    ),
                };
                let Some(planned) = planned else { continue };
                fixes.push(ClearanceFix {
                    glyph: glyph.clone(),
                    item_idx,
                    compose_idx,
                    old_line: compose.format_line(),
                    new_line: planned.line,
                    before: planned.before,
                    after: planned.after,
                    mismatched: planned.mismatched,
                    glyphs_warning: planned.glyphs_warning,
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

/// A name the optimizer is willing to reason about as one glyph: spelled out,
/// with no pattern and no `$` in it.
///
/// A block whose *name* is one of those is a family, and is planned by
/// [`optimize_pattern_line`] instead. Inside the inventory the test is what it
/// says: a name that is not one glyph names no drawing to measure.
fn is_plain_name(name: &str) -> bool {
    !crate::pattern::is_name_pattern(name) && !name.contains('$')
}

/// Every glyph a `glyph` block declares, as a part this pass could measure.
///
/// A block whose name is a pattern draws all of them with the one grid it
/// holds ([`crate::document::expand_glyph_block`]), so each of its names names
/// the same drawing — and a component naming one of them is the common case in
/// a Han source, where a radical's variants are written as one block per size.
/// A name still standing on a `$` after the base bindings have been applied is
/// no name at all and declares nothing here.
fn block_names(display: &str, name_parts: &crate::document::NamePartsMap) -> Vec<String> {
    if is_plain_name(display) {
        return vec![display.to_string()];
    }
    let substituted = crate::document::substitute_name_parts(display, name_parts);
    if substituted.contains('$') {
        return Vec::new();
    }
    let Ok(pattern) = crate::pattern::NamePattern::parse(&substituted) else {
        return Vec::new();
    };
    (0..pattern.len())
        .map(|i| crate::document::parse_glyph_name(&pattern.get(i)).display())
        .collect()
}

/// One part's ink, and the box [`InkProfile::of`] measures it over.
struct PartGrid<'a> {
    /// Borrowed for a part drawn by its own pixels; owned for one flattened
    /// out of a composite ([`Inventory::flatten_composites`]).
    grid: std::borrow::Cow<'a, PixelGrid>,
    scale: u8,
    origin: (i16, i16),
    extent: (u16, u16),
    /// The raster coordinate of the grid's cell `(0, 0)`, which is `(0, 0)`
    /// for a drawn grid and the flattening's own origin for a composite.
    raster: (i32, i32),
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
    fn collect(docs: &[&'a Document], name_parts: &crate::document::NamePartsMap) -> Self {
        let mut inv = Self {
            boxes: HashMap::new(),
            grids: HashMap::new(),
            variants: HashMap::new(),
            aliases: crate::alias::AliasMap::collect(docs, name_parts),
            profiles: std::cell::RefCell::new(HashMap::new()),
        };
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Glyph { name, body } = item else {
                    continue;
                };
                for name in block_names(&name.display(), name_parts) {
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
                                grid: std::borrow::Cow::Borrowed(pixels),
                                scale: body.scale,
                                origin: body.declared_origin(),
                                extent,
                                raster: (0, 0),
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
        }
        // A `glyph A = B` declares no drawing of its own, but it does declare a
        // second *name* for one — and the name is what states which slot the
        // drawing is for. `han-961d:4x16-c = han-961d:4x16-r` is the only way
        // the source says that the right-hand 阝 is what a ⿲'s middle slot
        // draws, so a family known by its blocks alone leaves that slot with no
        // candidate at all: every name it declares outright ranks as the wrong
        // direction there. The box and the ink come from the target, which is
        // what `canonical` already resolves every candidate through.
        let aliased: Vec<String> = inv
            .aliases
            .entries()
            .filter(|(name, target)| {
                !inv.boxes.contains_key(*name) && inv.boxes.contains_key(*target)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in aliased {
            if let Some((base, _)) = name.split_once(':') {
                inv.variants.entry(base.to_string()).or_default().push(name);
            }
        }
        for names in inv.variants.values_mut() {
            names.sort();
        }
        inv.flatten_composites(docs, name_parts);
        inv
    }

    /// Give the parts that draw no pixels of their own a grid to be measured
    /// over, by flattening them the way the build does
    /// ([`crate::ref_composite::resolve_reachable`]) — a radical written as a
    /// `ref` to a shared drawing is a candidate like any other, and so is one
    /// split by an IDC line of its own (`⿱艹林`, where 林 is `⿰木木`), whose
    /// line that walk derives before flattening it. One that could not be
    /// measured could not be chosen.
    ///
    /// The same walk the clearance *check* makes, deliberately: the fixer may
    /// only touch a line the check reports, so a part it can measure and the
    /// check cannot would let it rewrite a line nothing complained about. Two
    /// things narrow it further, both in the safe direction — a block whose
    /// name is a pattern is left out, since its `ref` names expand per glyph and
    /// the block holds only the unexpanded ones; and only the families some IDC
    /// line actually names are flattened, since nothing else can be chosen.
    fn flatten_composites(
        &mut self,
        docs: &[&'a Document],
        name_parts: &crate::document::NamePartsMap,
    ) {
        let mut families: std::collections::HashSet<String> = std::collections::HashSet::new();
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Glyph { body, .. } = item else {
                    continue;
                };
                for part in body.compose.iter().flat_map(|c| c.part_names()) {
                    let canonical = self.canonical(part);
                    let base = canonical.split_once(':').map_or(&canonical[..], |(b, _)| b);
                    families.insert(base.to_string());
                }
            }
        }
        if families.is_empty() {
            return;
        }

        // Every plain block's body, for the walk to follow refs through.
        let mut bodies: HashMap<String, &'a crate::document::GlyphBody> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Glyph { name, body } = item else {
                    continue;
                };
                let name = name.display();
                if !is_plain_name(&name) || bodies.contains_key(&name) {
                    continue;
                }
                if !body.refs.is_empty() || !body.compose.is_empty() {
                    let base = name.split_once(':').map_or(&name[..], |(b, _)| b);
                    if families.contains(base) {
                        roots.push(name.clone());
                    }
                }
                bodies.insert(name, body);
            }
        }
        if roots.is_empty() {
            return;
        }

        let resolved = crate::ref_composite::resolve_reachable(
            roots.iter().map(String::as_str),
            &|name| bodies.get(name).copied(),
            &self.aliases,
            name_parts,
        );
        for name in roots {
            let (Some(body), Some(flat)) = (bodies.get(&name), resolved.get(&name)) else {
                continue;
            };
            let extent = body.declared_extent().unwrap_or_else(|| {
                let s = flat.scale.max(1) as u16;
                (flat.grid.width / s, flat.grid.height / s)
            });
            self.grids.insert(
                name,
                PartGrid {
                    grid: std::borrow::Cow::Owned(flat.grid.clone()),
                    scale: flat.scale,
                    origin: body.declared_origin(),
                    extent,
                    raster: (flat.origin_col, flat.origin_row),
                },
            );
        }
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
        let computed = self.grids.get(name).map(|g| {
            Rc::new(InkProfile::of(
                &g.grid, g.scale, g.raster, g.origin, g.extent,
            ))
        });
        self.profiles
            .borrow_mut()
            .insert(name.to_string(), computed.clone());
        computed
    }

    /// One name, ready to be put in a slot — or `None` when it cannot go there.
    fn candidate(
        &self,
        written: &str,
        slot: Option<Direction>,
        cross: u16,
        horizontal: bool,
    ) -> Option<Candidate> {
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
            // Ranked on the name as *written*, which is the name the check
            // reads when it decides whether to warn.
            rank: crate::compose::direction_rank(written, slot),
            name: written.to_string(),
            extent: along as i32,
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
        along: i32,
        horizontal: bool,
    ) -> Vec<Candidate> {
        let mut out = Vec::new();
        let canonical = self.canonical(current);
        let base = if crate::compose::is_undecided(&canonical) {
            canonical.clone()
        } else {
            let Some(mine) = self.candidate(current, slot, cross, horizontal) else {
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
            // Two names for one drawing are two candidates on purpose — they
            // rank differently for the slot — so what is already offered is
            // matched on the name as written, and only the name the component
            // itself resolves to is dropped outright.
            if *name == canonical
                || out
                    .iter()
                    .any(|c| c.name == *name || self.canonical(&c.name) == *name)
            {
                continue;
            }
            // A drawing made for the other side of the glyph is not an
            // alternative for this slot; see `compose::direction_rank`.
            if crate::compose::direction_rank(name, slot) > 1 {
                continue;
            }
            if let Some(candidate) = self.candidate(name, slot, cross, horizontal) {
                // A drawing that would fill the glyph's own axis leaves the
                // rest of the line nowhere to stand; see `fits_beside`.
                if !fits_beside(candidate.extent, along) {
                    continue;
                }
                out.push(candidate);
            }
        }
        out
    }
}

/// One name a slot could hold, with everything the score needs from it.
///
/// Cloning one is cheap — the profile behind it is shared — which is what lets
/// a pattern line keep one per member of the family per label it considers.
#[derive(Clone)]
struct Candidate {
    name: String,
    /// The box's extent along the split axis, in declared units.
    extent: i32,
    frontier: AxisFrontier,
    /// How the name suits the slot it is being considered for, as
    /// [`crate::compose::direction_rank`] scores it: 0 the slot's own
    /// direction, 1 an unmarked name, 2 the wrong one — which is exactly the
    /// case `compose` warns about.
    rank: u8,
    profile: Rc<InkProfile>,
}

/// How the optimizer orders two answers. Derived `Ord` is the whole rule: the
/// fields are in the order the module docs list them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    /// How far outside the range the layout is, summed. The objective.
    score: i32,
    /// How many components are drawn for a slot other than the one they sit
    /// in — the second objective, and the only one that moves a line whose
    /// clearances are already perfect.
    mismatched: usize,
    /// More names drawn *for* their slot first.
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

/// Plan one IDC line, or `None` when the line does not warn, cannot be
/// measured, or cannot be improved.
fn optimize_line(
    inv: &Inventory,
    parent: (u16, u16),
    compose: &GlyphCompose,
    lo: i32,
    hi: i32,
    contact: Option<u16>,
) -> Option<PlannedLine> {
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

    // As written: the parts where the line's own gaps put them, and how many
    // of them are drawn for a slot they do not sit in. Both are things the
    // check warns about, and a line is left alone only when neither does.
    let before = match undecided {
        true => None,
        false => {
            let current: Vec<Candidate> = written
                .iter()
                .enumerate()
                .map(|(slot, name)| {
                    inv.candidate(name, op.slot_direction(slot), cross_extent, horizontal)
                })
                .collect::<Option<Vec<_>>>()?;
            let placed = walk(compose, &current);
            let as_written: Vec<&Candidate> = current.iter().collect();
            let before = (
                score(
                    &clearances_at(&as_written, &placed, axis_extent, horizontal, contact)?,
                    lo,
                    hi,
                ),
                current.iter().filter(|c| c.rank == 2).count(),
            );
            if before == (0, 0) {
                return None; // nothing warns, so nothing to fix
            }
            Some(before)
        }
    };

    let slots: Vec<Vec<Candidate>> = written
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            inv.candidates(
                name,
                op.slot_direction(slot),
                cross_extent,
                axis_extent,
                horizontal,
            )
        })
        .collect();
    let lengths: Vec<usize> = slots.iter().map(Vec::len).collect();
    let combinations: usize = lengths.iter().product();
    if combinations == 0 || combinations > MAX_COMBINATIONS {
        return None;
    }

    let mut best: Option<(Key, Vec<usize>)> = None;
    for pick in Combinations::new(&lengths) {
        let chosen: Vec<&Candidate> = pick
            .iter()
            .enumerate()
            .map(|(s, &i)| &slots[s][i])
            .collect();
        let Some(key) = evaluate(&chosen, &written, axis_extent, horizontal, lo, hi, contact)
        else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| key < *b) {
            best = Some((key, pick));
        }
    }
    let (key, pick) = best?;
    if before.is_some_and(|before| (key.score, key.mismatched) >= before) {
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
        let facing = effective_facing(&pair[0].profile, &pair[1].profile, horizontal, contact)?;
        at += key.clearances[i + 1] - facing;
        positions.push(at);
    }

    let line = write_line(compose, &chosen, &positions)?;
    // The arithmetic says what this layout measures; the measurement says so
    // too, or the line is left alone. Cheap, and the alternative is a command
    // that quietly writes a layout it was wrong about.
    let verified = clearances_at(&chosen, &positions, axis_extent, horizontal, contact)?;
    if score(&verified, lo, hi) != key.score {
        return None;
    }
    Some(PlannedLine {
        line,
        before: before.map(|(score, _)| score),
        after: key.score,
        mismatched: before.map(|(_, mismatched)| (mismatched, key.mismatched)),
        glyphs_warning: None,
    })
}

/// What planning one IDC line comes to: the line to write in its place, the
/// score before it and after, and — for a line that stands for a family — how
/// many of its glyphs warn on each side.
/// What planning one line comes to, whichever of the two planners did it.
/// The fields are [`ClearanceFix`]'s, minus the ones naming where the line is.
struct PlannedLine {
    line: String,
    before: Option<i32>,
    after: i32,
    mismatched: Option<(usize, usize)>,
    glyphs_warning: Option<(usize, usize)>,
}

/// One glyph of the family a pattern line stands for: what it is held to, and
/// the components it writes once the block's pattern has been expanded.
///
/// The names are the glyph's own, and every slot the family shares a label on
/// ([`LabelChoice`]) rewrites all of them at once.
struct MemberNames {
    lo: i32,
    hi: i32,
    /// One per slot, in the line's order.
    names: Vec<String>,
    contact: Option<u16>,
}

/// One glyph of the family at one choice of labels: the range its own rule
/// holds it to, and its layout as a function of the gaps.
///
/// The clearances are affine in the gaps and are kept that way: `base[i]` is
/// what clearance `i` is before the gaps are added (`c_i = gap_i + base[i]`),
/// and `total` is the sum every layout of these parts has, whatever the gaps
/// do — the same telescoping the module docs derive — so the last clearance is
/// `total` less the others. That is what makes one combination cost a handful
/// of additions per glyph, which matters when one line stands for thousands.
struct Member {
    lo: i32,
    hi: i32,
    base: Vec<i32>,
    total: i32,
    /// The `audit max-contact-run` rule this glyph is held to, for the
    /// verification pass; the layout itself already has it folded into `base`
    /// and `total`.
    contact: Option<u16>,
}

impl Member {
    /// The clearances the gaps `gaps` leave, into a buffer the caller reuses:
    /// one line stands for thousands of glyphs and is scored once per set of
    /// gaps, so this is the innermost loop there is here.
    fn clearances_into(&self, gaps: &[i32], out: &mut Vec<i32>) {
        out.clear();
        out.extend(gaps.iter().zip(&self.base).map(|(gap, base)| gap + base));
        out.push(self.total - out.iter().sum::<i32>());
    }

    fn clearances(&self, gaps: &[i32]) -> Vec<i32> {
        let mut out = Vec::with_capacity(self.base.len() + 1);
        self.clearances_into(gaps, &mut out);
        out
    }
}

/// One answer for a slot of a pattern line: a variant label the whole family
/// can carry, and what each of its glyphs draws when it does.
struct LabelChoice {
    /// The label this choice puts on the slot, and the name the line would
    /// write with it. `None` is the line's own component, left exactly as
    /// written — the only choice a slot whose label the block's own pattern
    /// reaches ever has.
    relabel: Option<(String, String)>,
    /// How the name suits the slot, as [`crate::compose::direction_rank`]
    /// scores it. One number for the family: the label is what carries a
    /// direction, and the label is what every glyph here shares.
    rank: u8,
    /// The glyph each member puts in the slot, in `members` order.
    parts: Vec<Candidate>,
}

/// Plan a pattern block's IDC line.
///
/// One line here stands for a family, and what a rewrite may move is what the
/// family *shares*: the gaps, and — this is [`slot_choices`] — a component's
/// variant label whenever the block's own pattern does not reach it. A
/// component written `(rx|ry):5x4` says the same `5x4` for every glyph the
/// block declares, so that label is the family's answer and not one glyph's,
/// and each glyph's own family is asked for the same label in turn. A
/// component written `rx:(4|5)x4` says something different per glyph and is
/// left alone. The *base* is never searched: a name is one glyph's answer.
///
/// The objective is that shared choice, in this order:
///
/// 1. **the fewest glyphs warning at all.** A family in which one more glyph is
///    finished is worth more than one in which every glyph is slightly less
///    wrong: the warnings are a work queue, and its length is what the command
///    is there to shorten;
/// 2. then the summed score, and the same tie-breaks a single line is ordered
///    by ([`Key`]) summed over the family — a gap moves every glyph's first
///    clearance by the same amount, so ordering the gaps lexicographically
///    orders the clearances the way [`Key`] does.
///
/// A glyph whose parts this pass cannot measure — a name nothing defines, a
/// part that is itself a composite, a component with no variant picked, a name
/// no `audit ideal-clearance` rule reaches — is left out of the answer rather
/// than making the whole family unfixable. Which glyphs those are is decided
/// once, on the line as written, so that every choice below is scored over the
/// same family; a *choice* that some member of it cannot be measured at is
/// dropped instead. The line still has to warn about something, and the answer
/// still has to improve on what is written.
fn optimize_pattern_line(
    inv: &Inventory,
    audit: &crate::audit::AuditRules,
    name_parts: &crate::document::NamePartsMap,
    glyph: &str,
    scale: u8,
    parent: (u16, u16),
    compose: &GlyphCompose,
) -> Option<PlannedLine> {
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

    // The same expansion the build does, so that what is optimized is what is
    // built: the block's name drives the count and each component pattern is
    // consumed in lock-step with it.
    let mut substituted = compose.clone();
    for item in &mut substituted.items {
        if let ComposeItem::Part { name, .. } = item {
            *name = crate::document::substitute_name_parts(name, name_parts);
        }
    }
    let body = GlyphBody {
        compose: vec![substituted],
        scale,
        ..GlyphBody::new()
    };
    let expanded = crate::document::expand_glyph_block(
        &crate::document::GlyphName(crate::document::substitute_name_parts(glyph, name_parts)),
        &body,
    )
    .ok()?;

    let mut members: Vec<MemberNames> = Vec::new();
    // The parts of each member as the line writes them, kept beside it: they
    // are the first choice of every slot below.
    let mut as_written: Vec<Vec<Candidate>> = Vec::new();
    for item in &expanded {
        let DocumentItem::Glyph { name, body } = item else {
            continue;
        };
        let member_name = name.display();
        let Some((_, lo, hi)) = audit.ideal_clearance.for_glyph(&member_name) else {
            continue;
        };
        let Some(line) = body.compose.first() else {
            continue;
        };
        let names: Vec<String> = line.part_names().map(str::to_string).collect();
        if names.len() != written.len() {
            continue;
        }
        // An undecided component names no drawing, here as anywhere: the glyph
        // has no layout to measure and waits for its author, while the rest of
        // the family is optimized around it.
        if names.iter().any(|n| crate::compose::is_undecided(n)) {
            continue;
        }
        let Some(parts) = names
            .iter()
            .enumerate()
            .map(|(slot, n)| inv.candidate(n, op.slot_direction(slot), cross_extent, horizontal))
            .collect::<Option<Vec<Candidate>>>()
        else {
            continue;
        };
        let contact = audit
            .max_contact_run
            .for_glyph(&member_name)
            .map(|(_, m)| m);
        // Two neighbours with no line on which both draw: nothing to measure.
        let refs: Vec<&Candidate> = parts.iter().collect();
        if affine_layout(&refs, axis_extent, horizontal, contact).is_none() {
            continue;
        }
        members.push(MemberNames {
            lo: lo as i32,
            hi: hi as i32,
            names,
            contact,
        });
        as_written.push(parts);
    }
    if members.is_empty() {
        return None;
    }

    let slots: Vec<Vec<LabelChoice>> = (0..written.len())
        .map(|slot| {
            slot_choices(
                inv,
                &members,
                &as_written,
                written[slot],
                slot,
                op.slot_direction(slot),
                cross_extent,
                axis_extent,
                horizontal,
            )
        })
        .collect();
    let lengths: Vec<usize> = slots.iter().map(Vec::len).collect();
    let label_combinations: usize = lengths.iter().product();
    if label_combinations == 0 || label_combinations > MAX_COMBINATIONS {
        return None;
    }

    // The gaps as written: everything before each component, summed, since a
    // line may write two numbers in a row. A trailing one moves nothing.
    let mut written_gaps: Vec<i32> = Vec::new();
    let mut pending = 0i32;
    for item in &compose.items {
        match item {
            ComposeItem::Gap(gap) => pending += *gap as i32,
            ComposeItem::Part { .. } => {
                written_gaps.push(std::mem::take(&mut pending));
            }
        }
    }

    let unchanged = vec![0usize; slots.len()];
    let written_members = member_layouts(&members, &slots, &unchanged, axis_extent, horizontal)?;
    let before = evaluate_gaps(
        &written_members,
        &slots,
        &unchanged,
        &written_gaps,
        &written_gaps,
    );
    if before.warnings == 0 && before.mismatched == 0 {
        return None; // nothing warns, so nothing to fix
    }

    let mut best: Option<(PatternKey, Vec<usize>, Vec<i32>)> = None;
    // A family is scored whole, once per set of gaps and per choice of labels,
    // so what a line costs is the product of the three. A line that would cost
    // more than the budget is left alone rather than allowed to take a minute,
    // exactly as an over-large variant family is above.
    let mut work = 0usize;
    for pick in Combinations::new(&lengths) {
        let Some(family) = member_layouts(&members, &slots, &pick, axis_extent, horizontal) else {
            continue; // a choice some glyph of the family cannot be measured at
        };
        // What each gap could usefully be: enough to put that clearance inside
        // some glyph's range, and one cell either side of that. Outside the
        // hull every glyph's clearance `i` is on the same side of its range, so
        // stepping back towards it gains each glyph a cell there and costs it
        // at most the one it takes from the last clearance — never a worse
        // answer, and often a better one. The gaps as written are in the set
        // whatever it says, so that "as written" is always one of the answers
        // compared.
        let mut choices: Vec<Vec<i32>> = Vec::with_capacity(written_gaps.len());
        for (i, gap) in written_gaps.iter().enumerate() {
            let (mut lo, mut hi) = (*gap, *gap);
            for member in &family {
                lo = lo.min(member.lo - member.base[i] - 1);
                hi = hi.max(member.hi - member.base[i] + 1);
            }
            choices.push((lo..=hi).collect());
        }
        let combinations: usize = choices.iter().map(Vec::len).product();
        if combinations == 0 {
            continue;
        }
        work = work.saturating_add(combinations.saturating_mul(family.len()));
        if work > MAX_PATTERN_WORK {
            return None;
        }

        let mut gaps = vec![0i32; choices.len()];
        for mut counter in 0..combinations {
            for (slot, values) in choices.iter().enumerate() {
                gaps[slot] = values[counter % values.len()];
                counter /= values.len();
            }
            let key = evaluate_gaps(&family, &slots, &pick, &gaps, &written_gaps);
            if best.as_ref().is_none_or(|(b, _, _)| key < *b) {
                best = Some((key, pick.clone(), gaps.clone()));
            }
        }
    }
    let (key, pick, gaps) = best?;
    if (key.warnings, key.score, key.mismatched)
        >= (before.warnings, before.score, before.mismatched)
    {
        return None; // a line nobody can improve keeps its warnings
    }

    let line = write_pattern_line(compose, &gaps, &slots, &pick)?;
    // What the arithmetic says the layout measures, measured. Same rule as
    // [`optimize_line`]: a command that quietly writes a layout it was wrong
    // about is worse than one that writes nothing.
    let family = member_layouts(&members, &slots, &pick, axis_extent, horizontal)?;
    let (mut score_after, mut warnings_after) = (0, 0);
    for (m, member) in family.iter().enumerate() {
        let parts = parts_at(&slots, &pick, m);
        let mut positions = vec![gaps[0]];
        for (i, part) in parts.iter().enumerate().take(parts.len() - 1) {
            positions.push(positions[i] + part.extent + gaps[i + 1]);
        }
        let measured = clearances_at(&parts, &positions, axis_extent, horizontal, member.contact)?;
        if measured != member.clearances(&gaps) {
            return None;
        }
        let s = score(&measured, member.lo, member.hi);
        score_after += s;
        warnings_after += usize::from(s > 0);
    }
    if (warnings_after, score_after) != (key.warnings, key.score) {
        return None;
    }
    Some(PlannedLine {
        line,
        before: Some(before.score),
        after: key.score,
        mismatched: Some((before.mismatched, key.mismatched)),
        glyphs_warning: Some((before.warnings, key.warnings)),
    })
}

/// Everything one slot of a pattern line could carry, the line's own component
/// first.
///
/// A label is a candidate only when the block's pattern does not reach it —
/// the line writes it as one string, and it means the same thing in every
/// glyph the block declares — and only when *every* glyph of the family draws
/// something at it. A family whose members' variants do not all offer the same
/// label offers no choice at all, which is the answer for a slot whose glyphs
/// are drawn one by one.
#[allow(clippy::too_many_arguments)]
fn slot_choices(
    inv: &Inventory,
    members: &[MemberNames],
    as_written: &[Vec<Candidate>],
    written: &str,
    slot: usize,
    dir: Option<Direction>,
    cross: u16,
    along: i32,
    horizontal: bool,
) -> Vec<LabelChoice> {
    let mut out = vec![LabelChoice {
        relabel: None,
        // Ranked on the name as *written*, which is the name the check reads
        // when it decides whether to warn.
        rank: crate::compose::direction_rank(written, dir),
        parts: as_written.iter().map(|p| p[slot].clone()).collect(),
    }];
    let Some((base, label)) = written.split_once(':') else {
        return out; // undecided: no label to move, and no drawing to move it to
    };
    if !is_plain_name(label) {
        return out; // a label the block's own pattern reaches
    }
    let suffix = format!(":{label}");
    // Each glyph's own base, twice over: as the line's own pattern expands it,
    // which is what a rewrite writes, and canonically — through the aliases,
    // exactly as [`Inventory::candidates`] does — which is the family a variant
    // is looked for in.
    let mut bases: Vec<(String, String)> = Vec::with_capacity(members.len());
    for member in members {
        let Some(written_base) = member.names[slot].strip_suffix(&suffix) else {
            return out; // the line's label is not this glyph's after all
        };
        let canonical = inv.canonical(&member.names[slot]);
        let Some((canonical_base, _)) = canonical.split_once(':') else {
            return out;
        };
        bases.push((written_base.to_string(), canonical_base.to_string()));
    }
    // The labels every one of them offers, and no more: one label has to serve
    // the whole family.
    let mut shared: Option<std::collections::BTreeSet<&str>> = None;
    for (_, base) in &bases {
        let mine: std::collections::BTreeSet<&str> = inv
            .variants
            .get(base)
            .into_iter()
            .flatten()
            .filter_map(|name| name.split_once(':').map(|(_, l)| l))
            .collect();
        shared = Some(match shared {
            None => mine,
            Some(prev) => prev.intersection(&mine).copied().collect(),
        });
        if shared.as_ref().is_some_and(|s| s.is_empty()) {
            return out;
        }
    }
    for candidate_label in shared.into_iter().flatten() {
        if out.len() >= MAX_CANDIDATES {
            break;
        }
        if candidate_label == label {
            continue; // the line's own, already first
        }
        let name = format!("{base}:{candidate_label}");
        // A drawing made for the other side of the glyph is not an alternative
        // for this slot; see `compose::direction_rank`.
        let rank = crate::compose::direction_rank(&name, dir);
        if rank > 1 {
            continue;
        }
        // Measured as the line would write it — a base whose alias exists at
        // one label only would otherwise be relabelled into a name nothing
        // defines — and only where that is the same glyph the family offered.
        let Some(parts) = bases
            .iter()
            .map(|(written_base, base)| {
                let name = format!("{written_base}:{candidate_label}");
                let part = inv.candidate(&name, dir, cross, horizontal)?;
                (inv.canonical(&name) == format!("{base}:{candidate_label}")).then_some(part)
            })
            .collect::<Option<Vec<Candidate>>>()
        else {
            continue; // some glyph of the family draws nothing at this label
        };
        // One glyph the label would not fit in is enough: the label is the
        // family's answer and every glyph of it has to be able to hold it.
        if parts.iter().any(|part| !fits_beside(part.extent, along)) {
            continue;
        }
        out.push(LabelChoice {
            relabel: Some((candidate_label.to_string(), name)),
            rank,
            parts,
        });
    }
    out
}

/// The parts one member of the family puts in its slots, at one choice of
/// labels.
fn parts_at<'a>(
    slots: &'a [Vec<LabelChoice>],
    pick: &[usize],
    member: usize,
) -> Vec<&'a Candidate> {
    slots
        .iter()
        .zip(pick)
        .map(|(choices, &i)| &choices[i].parts[member])
        .collect()
}

/// One glyph's clearances as a function of the gaps: `(base, total)`, the
/// affine form [`Member`] documents. `None` when two neighbours share no line
/// on which both draw.
fn affine_layout(
    parts: &[&Candidate],
    axis_extent: i32,
    horizontal: bool,
    contact: Option<u16>,
) -> Option<(Vec<i32>, i32)> {
    let last = parts.len() - 1;
    let mut base = vec![parts[0].frontier.near];
    let mut total = parts[0].frontier.near + (axis_extent - 1 - parts[last].frontier.far);
    for pair in parts.windows(2) {
        let facing = effective_facing(&pair[0].profile, &pair[1].profile, horizontal, contact)?;
        base.push(pair[0].extent + facing);
        total += facing;
    }
    Some((base, total))
}

/// The whole family's layouts at one choice of labels, or `None` when some
/// glyph of it cannot be measured there — a choice that is no answer, since
/// every choice is scored over the same family.
fn member_layouts(
    members: &[MemberNames],
    slots: &[Vec<LabelChoice>],
    pick: &[usize],
    axis_extent: i32,
    horizontal: bool,
) -> Option<Vec<Member>> {
    members
        .iter()
        .enumerate()
        .map(|(m, member)| {
            let parts = parts_at(slots, pick, m);
            let (base, total) = affine_layout(&parts, axis_extent, horizontal, member.contact)?;
            Some(Member {
                lo: member.lo,
                hi: member.hi,
                base,
                total,
                contact: member.contact,
            })
        })
        .collect()
}

/// How the optimizer orders two answers for a pattern line. Derived `Ord`
/// again, and the fields are [`Key`]'s with the family's own objective — how
/// many glyphs warn — in front, and each of the rest summed over the family.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PatternKey {
    /// How many of the glyphs the line stands for warn at all. The objective.
    warnings: usize,
    /// How far outside their ranges the family is, summed.
    score: i32,
    /// How many slots hold a label drawn for another slot — one number for the
    /// family, the label being what every glyph here shares.
    mismatched: usize,
    /// More labels drawn *for* their slot first.
    directed: std::cmp::Reverse<usize>,
    edge_sum: i32,
    inner_spread: i32,
    /// `false` — the line as written — sorts first.
    changed: bool,
    gaps: Vec<i32>,
    names: Vec<String>,
}

/// Score one choice of labels and gaps over the whole family.
fn evaluate_gaps(
    family: &[Member],
    slots: &[Vec<LabelChoice>],
    pick: &[usize],
    gaps: &[i32],
    written: &[i32],
) -> PatternKey {
    let chosen: Vec<&LabelChoice> = slots.iter().zip(pick).map(|(c, &i)| &c[i]).collect();
    let mut key = PatternKey {
        warnings: 0,
        score: 0,
        mismatched: chosen.iter().filter(|c| c.rank == 2).count(),
        directed: std::cmp::Reverse(chosen.iter().filter(|c| c.rank == 0).count()),
        edge_sum: 0,
        inner_spread: 0,
        changed: gaps != written || chosen.iter().any(|c| c.relabel.is_some()),
        gaps: gaps.to_vec(),
        names: chosen
            .iter()
            .map(|c| match &c.relabel {
                Some((_, name)) => name.clone(),
                None => String::new(),
            })
            .collect(),
    };
    let mut clearances: Vec<i32> = Vec::with_capacity(4);
    for member in family {
        member.clearances_into(gaps, &mut clearances);
        let n = clearances.len();
        let s = score(&clearances, member.lo, member.hi);
        key.warnings += usize::from(s > 0);
        key.score += s;
        key.edge_sum += clearances[0] + clearances[n - 1];
        if n == 4 {
            key.inner_spread += (clearances[1] - clearances[2]).abs();
        }
    }
    key
}

/// The line with `gaps` in place of the ones it writes and the chosen labels in
/// place of the ones its components carry, and everything else — the operator,
/// the components' base names as the block spells them, the comment — left
/// exactly as it is. A pattern line's component *names* are the family's and
/// not this pass's to choose; only the label they share is.
fn write_pattern_line(
    compose: &GlyphCompose,
    gaps: &[i32],
    slots: &[Vec<LabelChoice>],
    pick: &[usize],
) -> Option<String> {
    let mut items: Vec<ComposeItem> = Vec::new();
    let mut parts = compose
        .items
        .iter()
        .filter(|i| matches!(i, ComposeItem::Part { .. }));
    for (slot, gap) in gaps.iter().enumerate() {
        if *gap != 0 {
            items.push(ComposeItem::Gap(i16::try_from(*gap).ok()?));
        }
        let part = parts.next()?;
        let ComposeItem::Part { raw_name, .. } = part else {
            return None;
        };
        items.push(match &slots.get(slot)?.get(pick[slot])?.relabel {
            // The block's own component, untouched: the `@` form and all.
            None => part.clone(),
            // Only the label moved, so the name is written the way the line
            // already writes it, with the new label in place of the old.
            Some((label, name)) => ComposeItem::Part {
                name: name.clone(),
                raw_name: raw_name
                    .as_deref()
                    .and_then(|raw| raw.split_once(':'))
                    .map(|(raw_base, _)| format!("{raw_base}:{label}")),
            },
        });
    }
    Some(
        GlyphCompose {
            op: compose.op,
            items,
            comment: compose.comment.clone(),
        }
        .format_line(),
    )
}

/// Score one candidate combination, and the layout it is scored at.
fn evaluate(
    chosen: &[&Candidate],
    written: &[&str],
    axis_extent: i32,
    horizontal: bool,
    lo: i32,
    hi: i32,
    contact: Option<u16>,
) -> Option<Key> {
    let n = chosen.len() + 1;
    // The sum every layout of these variants has, whatever the gaps do.
    let mut total = chosen[0].frontier.near + (axis_extent - 1 - chosen[n - 2].frontier.far);
    for pair in chosen.windows(2) {
        total += effective_facing(&pair[0].profile, &pair[1].profile, horizontal, contact)?;
    }
    let clearances = arrange(n, total, lo, hi);
    Some(Key {
        score: score(&clearances, lo, hi),
        mismatched: chosen.iter().filter(|c| c.rank == 2).count(),
        directed: std::cmp::Reverse(chosen.iter().filter(|c| c.rank == 0).count()),
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
    contact: Option<u16>,
) -> Option<Vec<i32>> {
    let last = parts.len() - 1;
    let mut out = vec![positions[0] + parts[0].frontier.near];
    for i in 0..last {
        let facing = effective_facing(
            &parts[i].profile,
            &parts[i + 1].profile,
            horizontal,
            contact,
        )?;
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
            comment: compose.comment.clone(),
        }
        .format_line(),
    )
}

/// How far `v` is from the inclusive range; 0 inside it.
/// Whether a part this long may be *proposed* for a glyph whose own axis is
/// `along` long: [`crate::compose::fits_axis`], which is where the rule and the
/// reason for it live.
///
/// It is a bound on what may be *offered*, not on what may be written: the
/// component as the line already writes it stays a candidate whatever its size,
/// since it is the source's own choice rather than a proposal — the same rule
/// [`Inventory::candidates`] states for a drawing made for the wrong slot.
fn fits_beside(extent: i32, along: i32) -> bool {
    crate::compose::fits_axis(extent, along)
}

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

/// The cartesian product of the slots' candidate lists, as index vectors over
/// the lists' lengths. The last slot varies fastest, so the order is the one
/// the lists are written in.
struct Combinations {
    lengths: Vec<usize>,
    next: Option<Vec<usize>>,
}

impl Combinations {
    fn new(lengths: &[usize]) -> Self {
        let lengths = lengths.to_vec();
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
