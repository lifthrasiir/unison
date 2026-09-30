//! The inventory of glyphs a slot could be filled with, and what is known about
//! each: the split's [`Candidate`] and the enclosure's [`EnclosurePart`].

use super::MAX_CANDIDATES;
use super::layout::fits_beside;
#[cfg(doc)]
use super::names::slot_names;
use super::names::{
    block_names, expand_block, expand_block_lines, is_nested_slot, is_plain_name, slot_rank,
};
use crate::compose::{AxisFrontier, Direction, GapSide, InkProfile, VariantSpec};
use crate::document::{
    ComposeItem, Document, DocumentItem, GlyphBody, GlyphCompose, Margin, PixelGrid,
};
use crate::hash::HashMap;
use std::rc::Rc;

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
pub(super) struct Inventory<'a> {
    /// Declared box per glyph name; `None` for a glyph whose header declares
    /// no `W H`, which no component may be.
    boxes: HashMap<String, Option<(u16, u16)>>,
    /// The `margin-x` / `margin-y` of the glyphs that state one.
    margins: HashMap<String, Margin>,
    /// The grid of every glyph that draws itself entirely with its own pixels.
    /// A composite draws ink this pass cannot see, and half a part's ink
    /// measured is worse than none — the same rule `expand.rs::ink_profiles`
    /// applies.
    /// Name → the grid to measure and the box to measure it over.
    grids: HashMap<String, PartGrid<'a>>,
    /// Base name (everything before the first `:`) → its variants.
    pub(super) variants: HashMap<String, Vec<String>>,
    aliases: crate::alias::AliasMap,
    /// The reduction each anchor class states, so a composite flattened here
    /// places its refs exactly as the build does.
    aligns: crate::document::AnchorAligns,
    /// Memoized [`InkProfile`]s: one part is a component of hundreds of glyphs,
    /// and its profile is the same in every one of them.
    profiles: std::cell::RefCell<HashMap<String, Option<Rc<InkProfile>>>>,
}

impl<'a> Inventory<'a> {
    pub(super) fn collect(
        docs: &[&'a Document],
        name_parts: &crate::document::NamePartsMap,
        exists: &crate::exists::ExistsScopes,
    ) -> Self {
        let mut inv = Self {
            boxes: HashMap::default(),
            margins: HashMap::default(),
            grids: HashMap::default(),
            variants: HashMap::default(),
            aliases: crate::alias::AliasMap::collect_with_merges(docs, name_parts, exists),
            aligns: crate::document::collect_anchor_aligns(
                docs.iter().flat_map(|d| d.items.iter()),
            ),
            profiles: std::cell::RefCell::new(HashMap::default()),
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
                    if body.margin != Margin::default() {
                        inv.margins.insert(name.clone(), body.margin);
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
    /// check cannot would let it rewrite a line nothing complained about. The
    /// check reads the *expanded* source, so a block whose name is a pattern is
    /// expanded here first ([`expand_block`]) — both the ones that compose,
    /// since a Han source draws a regional family as one such block per size
    /// (`glyph han-5c3c-($han-regions):10x16` over `⿸尸匕`), and the lines
    /// that name one, since `han-5c3c-($-1)` names the family only once it is
    /// expanded. One thing narrows it, in the safe direction: only the families
    /// some IDC line actually names are flattened, since nothing else can be
    /// chosen.
    fn flatten_composites(
        &mut self,
        docs: &[&'a Document],
        name_parts: &crate::document::NamePartsMap,
    ) {
        // Every glyph block's body in document order, a pattern block's once per
        // glyph it declares. Only the ones that compose are expanded: a drawn
        // one is in `grids` under every name already, and has no line to name
        // a family with. A composite's names are canonicalized, as the
        // expansion's are: the walk follows a ref by the name it carries, so
        // `han-5315-j:6x16` has to have become the `han-5315.0:6x16` that is
        // declared before the glyph drawn from it can be flattened.
        let mut blocks: Vec<(String, std::borrow::Cow<'_, GlyphBody>)> = Vec::new();
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Glyph { name, body } = item else {
                    continue;
                };
                let name = name.display();
                if body.refs.is_empty() && body.compose.is_empty() {
                    if is_plain_name(&name) {
                        blocks.push((name, std::borrow::Cow::Borrowed(body)));
                    }
                    continue;
                }
                let expanded = match is_plain_name(&name) {
                    true => vec![(name, body.clone())],
                    false => expand_block(name_parts, &name, body).unwrap_or_default(),
                };
                for (name, mut body) in expanded {
                    for gref in &mut body.refs {
                        self.aliases.canonicalize(&mut gref.name);
                    }
                    for (part, _) in body.compose.iter_mut().flat_map(|c| c.parts_mut()) {
                        self.aliases.canonicalize(part);
                    }
                    blocks.push((name, std::borrow::Cow::Owned(body)));
                }
            }
        }
        let mut families: crate::hash::HashSet<String> = crate::hash::HashSet::default();
        for (_, body) in &blocks {
            for part in body.compose.iter().flat_map(|c| c.part_names()) {
                let base = part.split_once(':').map_or(part, |(b, _)| b);
                families.insert(base.to_string());
            }
        }
        if families.is_empty() {
            return;
        }

        let nested = self.register_nested(docs, name_parts);
        // Every block's body, for the walk to follow refs through.
        let mut bodies: HashMap<String, &crate::document::GlyphBody> = HashMap::default();
        let mut roots: Vec<String> = Vec::new();
        for (key, body) in &nested {
            bodies.insert(key.clone(), body);
            roots.push(key.clone());
        }
        for (name, body) in &blocks {
            if !is_plain_name(name) || bodies.contains_key(name) {
                continue;
            }
            if !body.refs.is_empty() || !body.compose.is_empty() {
                let base = name.split_once(':').map_or(&name[..], |(b, _)| b);
                if families.contains(base) {
                    roots.push(name.clone());
                }
            }
            bodies.insert(name.clone(), body);
        }
        if roots.is_empty() {
            return;
        }

        let resolved = crate::ref_composite::resolve_reachable(
            roots.iter().map(String::as_str),
            &|name| bodies.get(name).copied(),
            &self.aliases,
            name_parts,
            &self.aligns,
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

    /// Make up the glyph every nested split of a split line stands for, as
    /// the build does (`expand.rs::ink_profiles`): its box inferred from its
    /// members, recorded here under its slot name ([`slot_names`]), and its
    /// body ([`crate::compose::nested_body`]) handed back for
    /// [`Self::flatten_composites`] to flatten. A pattern block's lines are
    /// expanded first, since each glyph of it has a nested split of its own;
    /// one whose box cannot be inferred is not recorded and so cannot be
    /// measured, exactly as the check cannot measure it.
    fn register_nested(
        &mut self,
        docs: &[&Document],
        name_parts: &crate::document::NamePartsMap,
    ) -> Vec<(String, GlyphBody)> {
        let mut out: Vec<(String, GlyphBody)> = Vec::new();
        let mut seen: crate::hash::HashSet<String> = crate::hash::HashSet::default();
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Glyph { name, body } = item else {
                    continue;
                };
                for compose in &body.compose {
                    if compose.op.enclosing()
                        || !compose
                            .items
                            .iter()
                            .any(|it| matches!(it, ComposeItem::Nested(_)))
                    {
                        continue;
                    }
                    let glyph = name.display();
                    let lines: Vec<GlyphCompose> = match is_plain_name(&glyph) {
                        true => vec![compose.clone()],
                        false => expand_block_lines(name_parts, &glyph, body.scale, compose)
                            .into_iter()
                            .flatten()
                            .map(|(_, line)| line)
                            .collect(),
                    };
                    for line in &lines {
                        for it in &line.items {
                            let ComposeItem::Nested(members) = it else {
                                continue;
                            };
                            let key = crate::compose::nested_key(line.op, members);
                            if !seen.insert(key.clone()) {
                                continue;
                            }
                            let dims = |n: &str| self.dims(&self.canonical(n));
                            let Some(mut body) =
                                crate::compose::nested_body(line.op, members, &dims)
                            else {
                                continue;
                            };
                            // The walk expects canonical names, as the
                            // expansion's bodies carry.
                            for r in &mut body.refs {
                                r.name = self.canonical(&r.name);
                            }
                            out.push((key, body));
                        }
                    }
                }
            }
        }
        for (key, body) in &out {
            self.boxes.insert(key.clone(), body.extent);
        }
        out
    }

    /// What a canonical name declares, as the IDC layout reads it.
    fn dims(&self, canonical: &str) -> crate::compose::PartDims {
        match self.boxes.get(canonical) {
            None => crate::compose::PartDims::Unknown,
            Some(&size) => crate::compose::PartDims::declared(
                size,
                self.margins.get(canonical).copied().unwrap_or_default(),
            ),
        }
    }

    pub(super) fn canonical(&self, name: &str) -> String {
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
    pub(super) fn candidate(
        &self,
        written: &str,
        slot: Option<Direction>,
        cross: u16,
        horizontal: bool,
    ) -> Option<Candidate> {
        match self.candidate_state(written, slot, cross, horizontal) {
            SlotState::Ok(candidate) => Some(*candidate),
            _ => None,
        }
    }

    /// The same, saying *why* when the name cannot go in the slot: the two
    /// answers are not the same act. See [`SlotState`].
    pub(super) fn candidate_state(
        &self,
        written: &str,
        slot: Option<Direction>,
        cross: u16,
        horizontal: bool,
    ) -> SlotState {
        let canonical = self.canonical(written);
        // Every one of these is a `Severity::Error` on the line, and the
        // messages `compose::compose_refs` writes for them say what they are:
        // a component nothing defines, one whose header declares no box, one
        // that does not fill the slot across the axis, one whose name is wrong
        // about the size of the glyph it names.
        let crate::compose::PartDims::Size(w, h, margin) = self.dims(&canonical) else {
            return SlotState::Faulty;
        };
        let along = if horizontal { w } else { h };
        // Padded across the slot by its margin, as the line would lay it out.
        let Some(cross_at) = crate::compose::padding_across((w, h), margin, cross, horizontal)
        else {
            return SlotState::Faulty;
        };
        // A nested split's name states no size; its box is what its members
        // add up to ([`Inventory::register_nested`]).
        if !is_nested_slot(&canonical)
            && VariantSpec::parse(&canonical)
                .size
                .is_some_and(|size| size != (w, h))
        {
            return SlotState::Faulty;
        }
        // Past here the name is sound and the *drawing* is what is missing:
        // nothing is wrong with the line, this pass simply cannot read what it
        // would have to read to score it.
        let Some(profile) = self.profile(&canonical) else {
            return SlotState::Unmeasurable;
        };
        let Some(frontier) = profile.frontier(horizontal) else {
            return SlotState::Unmeasurable;
        };
        SlotState::Ok(Box::new(Candidate {
            frontier,
            // Ranked on the name as *written*, which is the name the check
            // reads when it decides whether to warn.
            rank: slot_rank(written, slot),
            name: written.to_string(),
            extent: along as i32,
            cross: cross_at as i32,
            profile,
        }))
    }

    /// Everything that could fill the slot `current` fills now, `current`
    /// itself first.
    ///
    /// A component that has not picked its variant yet is *not* itself a
    /// candidate — it names no drawing at all — and its whole name is the base
    /// whose family fills the slot instead. That is the one case where the list
    /// can come back without the name the line is written with.
    pub(super) fn candidates(
        &self,
        current: &str,
        slot: Option<Direction>,
        cross: u16,
        along: i32,
        horizontal: bool,
    ) -> Vec<Candidate> {
        let mut out = Vec::new();
        // One part with one candidate; see [`slot_names`].
        if is_nested_slot(current) {
            out.extend(self.candidate(current, slot, cross, horizontal));
            return out;
        }
        let canonical = self.canonical(current);
        let base = if crate::compose::is_undecided(&canonical) {
            canonical.clone()
        } else {
            match self.candidate(current, slot, cross, horizontal) {
                Some(mine) => {
                    out.push(mine);
                    let Some((base, _)) = canonical.split_once(':') else {
                        return out;
                    };
                    base.to_string()
                }
                // A name the check errors on: it names no drawing that could
                // fill the slot, so it is not offered back, and the family it
                // names is searched for one that could. That is the second case
                // where the list can come back without the name the line is
                // written with, and the caller has already stopped scoring the
                // line as written.
                None => canonical
                    .split_once(':')
                    .map_or_else(|| canonical.clone(), |(base, _)| base.to_string()),
            }
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

/// What a name written in a slot turns out to be.
///
/// The two ways it can fail are not the same thing and the caller does not
/// treat them alike: a **faulty** name is one the check reports a
/// `Severity::Error` for, so the line has no layout that was measured — there
/// is nothing to have got worse, and the family the component names is
/// searched for a name that would work. An **unmeasurable** one is a sound
/// name whose drawing this pass cannot read (a composite it could not flatten,
/// a part with no ink of its own), which is the case it has always skipped:
/// what it cannot measure before a choice it cannot measure after one either.
pub(super) enum SlotState {
    Ok(Box<Candidate>),
    Faulty,
    Unmeasurable,
}

/// One name a slot could hold, with everything the score needs from it.
///
/// Cloning one is cheap — the profile behind it is shared — which is what lets
/// a pattern line keep one per member of the family per label it considers.
#[derive(Clone)]
pub(super) struct Candidate {
    pub(super) name: String,
    /// The box's extent along the split axis, in declared units.
    pub(super) extent: i32,
    pub(super) frontier: AxisFrontier,
    /// How the name suits the slot it is being considered for, as
    /// [`crate::compose::direction_rank`] scores it: 0 the slot's own
    /// direction, 1 an unmarked name, 2 the wrong one — which is exactly the
    /// case `compose` warns about.
    pub(super) rank: u8,
    profile: Rc<InkProfile>,
    /// Where the part starts across the slot: 0, or the near side of the
    /// margin that pads it there.
    cross: i32,
}

impl Candidate {
    /// The part as one side of a gap, where it sits across the slot.
    pub(super) fn side(&self) -> GapSide<'_> {
        GapSide {
            cross: self.cross,
            ..GapSide::linear(&self.profile)
        }
    }
}

/// One name an enclosure slot could hold. The counterpart of [`Candidate`],
/// and different from it in exactly the way the two layouts are: an enclosure
/// part is placed on both axes, so what is kept is its whole box rather than
/// one extent and one axis's frontier.
#[derive(Clone)]
pub(super) struct EnclosurePart {
    pub(super) name: String,
    pub(super) size: (u16, u16),
    /// [`crate::compose::enclosure_rank`] for the slot it is considered for:
    /// 0 a drawing made for it, 1 one that claims nothing, 2 one made for the
    /// other slot — which is exactly the case `compose` warns about.
    pub(super) rank: u8,
    pub(super) profile: Rc<InkProfile>,
    /// Where the part sits when it is the outer one: `(0, 0)` but for a
    /// drawing its margin pads ([`crate::compose::outer_padding`]).
    pub(super) at: (i32, i32),
}

/// [`SlotState`] for an enclosure slot; the three answers mean the same things.
pub(super) enum EnclosureSlot {
    Ok(Box<EnclosurePart>),
    Faulty,
    Unmeasurable,
}

impl Inventory<'_> {
    /// One name, ready to be put in an enclosure slot.
    pub(super) fn enclosure_slot(
        &self,
        written: &str,
        parent: (u16, u16),
        outer: bool,
    ) -> EnclosureSlot {
        let canonical = self.canonical(written);
        let crate::compose::PartDims::Size(w, h, margin) = self.dims(&canonical) else {
            return EnclosureSlot::Faulty;
        };
        let spec = VariantSpec::parse(&canonical);
        if spec.size.is_some_and(|size| size != (w, h)) {
            return EnclosureSlot::Faulty;
        }
        if !crate::compose::fits_enclosure_slot((w, h), margin, parent, outer) {
            return EnclosureSlot::Faulty;
        }
        let at = match outer {
            true => crate::compose::outer_padding((w, h), margin, parent)
                .map_or((0, 0), |(x, y)| (x as i32, y as i32)),
            false => (0, 0),
        };
        let Some(profile) = self.profile(&canonical) else {
            return EnclosureSlot::Unmeasurable;
        };
        // Both axes, since an enclosure is measured on both: a part with ink on
        // one and none on the other is not something this pass can place.
        if profile.frontier(true).is_none() || profile.frontier(false).is_none() {
            return EnclosureSlot::Unmeasurable;
        }
        EnclosureSlot::Ok(Box::new(EnclosurePart {
            // Ranked on the name as *written*, which is the name the check
            // reads when it decides whether to warn.
            rank: crate::compose::enclosure_rank(written, outer),
            name: written.to_string(),
            size: (w, h),
            profile,
            at,
        }))
    }

    /// Everything that could fill the enclosure slot `current` fills now,
    /// `current` itself first — the same rule [`Self::candidates`] states, with
    /// the cavity in a name standing in for the direction letter.
    pub(super) fn enclosure_candidates(
        &self,
        current: &str,
        parent: (u16, u16),
        outer: bool,
    ) -> Vec<EnclosurePart> {
        let mut out: Vec<EnclosurePart> = Vec::new();
        let canonical = self.canonical(current);
        let base = if crate::compose::is_undecided(&canonical) {
            canonical.clone()
        } else {
            match self.enclosure_slot(current, parent, outer) {
                EnclosureSlot::Ok(mine) => {
                    out.push(*mine);
                    let Some((base, _)) = canonical.split_once(':') else {
                        return out;
                    };
                    base.to_string()
                }
                _ => canonical
                    .split_once(':')
                    .map_or_else(|| canonical.clone(), |(base, _)| base.to_string()),
            }
        };
        for name in self.variants.get(&base).into_iter().flatten() {
            if out.len() >= MAX_CANDIDATES {
                break;
            }
            if *name == canonical
                || out
                    .iter()
                    .any(|c| c.name == *name || self.canonical(&c.name) == *name)
            {
                continue;
            }
            // A drawing made for the other slot is not an alternative for this
            // one; see `compose::enclosure_rank`.
            if crate::compose::enclosure_rank(name, outer) > 1 {
                continue;
            }
            if let EnclosureSlot::Ok(candidate) = self.enclosure_slot(name, parent, outer) {
                out.push(*candidate);
            }
        }
        out
    }
}
