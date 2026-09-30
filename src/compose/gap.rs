//! The gap between two adjacent parts: `GapSide`, the facing offset, and the contact run and demand.

use super::ink::{Face, InkLine, InkProfile, covers_meet};

/// The clearance two adjacent parts would leave each other if the second sat
/// exactly at the first's own origin — so the clearance at any placement is
/// this plus the distance between the two origins.
///
/// `None` when the two share no line on which both draw: there is then no pair
/// of frontiers to measure between, and the line is not measured rather than
/// measured wrong.
///
/// A line's clearance is the gap between the two frontiers *plus* the depth the
/// parts' facing hardblank runs share ([`InkLine`]): where both sides wrote a
/// hardblank the space is written twice over, and space written twice is one
/// space. Only the shared depth counts — the smaller of the two runs — so
/// nothing is ever measured through a cell one side draws ink in. The result is
/// therefore this side of the frontier-only measurement: a pair that meets
/// hardblank to hardblank reads as *more* clear than its frontiers say, and so
/// sits that much closer once the ideal clearance is solved for.
pub fn facing_offset(lo: GapSide, hi: GapSide, horizontal: bool) -> Option<i32> {
    lo.shared_span(hi, horizontal)
        .filter_map(|line| {
            let f = lo.line_at(horizontal, line)?.upper(lo.inner)?;
            let g = hi.line_at(horizontal, line)?.lower(hi.inner)?;
            let shared = f.hardblanks.min(g.hardblanks) as i32;
            Some(g.at - f.at - 1 + shared)
        })
        .min()
}

/// One side of one gap: whose drawing bounds it, which of that drawing's
/// boundaries faces the gap, and where the drawing sits across the axis.
///
/// A one-dimensional line needs none of the last two — both parts fill the
/// parent's whole cross extent and present the ends everyone can see — which is
/// what [`GapSide::linear`] says. An enclosure needs both: its outer part
/// bounds the gap with the *inner* face of a wall, and its inner part sits
/// somewhere along the cross axis rather than spanning it.
#[derive(Clone, Copy, Debug)]
pub struct GapSide<'a> {
    pub profile: &'a InkProfile,
    /// Read the inner face — a wall's cavity side — rather than the outward
    /// one.
    pub inner: bool,
    /// Where this part's box starts along the *cross* axis, in the parent's
    /// declared cells, so that two parts' lines can be matched by the parent's
    /// coordinate rather than by index.
    pub cross: i32,
}

impl<'a> GapSide<'a> {
    /// A part of a one-dimensional line: its outward face, spanning the
    /// parent across the axis. Every measurement a `⿰`/`⿱`/`⿲`/`⿳` line makes
    /// is between two of these, and reads exactly the numbers it always did.
    pub fn linear(profile: &'a InkProfile) -> Self {
        Self {
            profile,
            inner: false,
            cross: 0,
        }
    }

    /// The stretch of the parent's cross axis both sides have a line on, as a
    /// half-open range of parent coordinates. Outside it one of the two parts
    /// simply is not there, and a gap between a drawing and nothing is not a
    /// gap.
    fn shared_span(self, other: GapSide<'a>, horizontal: bool) -> std::ops::Range<i32> {
        let (a, b) = (
            self.profile.along(horizontal).len() as i32,
            other.profile.along(horizontal).len() as i32,
        );
        let start = self.cross.max(other.cross);
        let end = (self.cross + a).min(other.cross + b);
        start..start.max(end)
    }

    /// This side's line at one parent cross coordinate, or `None` where it
    /// draws nothing there.
    fn line_at(self, horizontal: bool, line: i32) -> Option<InkLine> {
        let index = usize::try_from(line - self.cross).ok()?;
        *self.profile.along(horizontal).get(index)?
    }

    /// The covers of the boundary this side presents to the gap, per line,
    /// indexed the way `Self::paired_lines` walks them.
    fn facing_cover(
        self,
        horizontal: bool,
        upward: bool,
        index: usize,
    ) -> Option<&'a [(u16, u16)]> {
        let e = self.profile.along_edges(horizontal).get(index)?;
        Some(match (upward, self.inner) {
            (true, false) => &e.far,
            (true, true) => &e.first_far,
            (false, false) => &e.near,
            (false, true) => &e.last_near,
        })
    }
}

/// How many consecutive lines the two parts' ink touches along, with `b` sitting
/// `delta` declared cells past `a`'s own origin — the longest such run, since a
/// split is spoiled by one long seam rather than by scattered nicks.
///
/// A line counts when both parts draw ink on it and their *contours* meet:
/// the two frontier cells abut and the ink in them covers some of the boundary
/// they share ([`EdgeCover`](super::ink::EdgeCover)). Cells alone would be the stricter question and
/// the less intuitive one — a diagonal that ends in a corner inks its cell
/// without reaching the side of it, and two such cells pass each other with no
/// seam to speak of.
///
/// Either way it is a question about the *pattern* of two facing edges and not
/// about the distance between them, which is the whole point: two flat faces
/// meeting over sixteen lines and two tips grazing over one are the same
/// clearance and want opposite answers.
///
/// Hardblanks need no term here. A hardblank holds a part's *ink* frontier back
/// (see [`Face::ink`]), so a claim that has already parted two parts leaves
/// nothing touching for this to count, and the two mechanisms never both fire
/// on one line. A line either part draws nothing on breaks the run: there is no
/// contact where one side is not there.
pub fn contact_run(lo: GapSide, hi: GapSide, horizontal: bool, delta: i32) -> u16 {
    let (mut longest, mut run) = (0u16, 0u16);
    for line in lo.shared_span(hi, horizontal) {
        let faces = lo
            .line_at(horizontal, line)
            .zip(hi.line_at(horizontal, line));
        let touching = match faces
            .map(|(x, y)| {
                (
                    x.upper(lo.inner).and_then(Face::ink),
                    y.lower(hi.inner).and_then(Face::ink),
                )
            })
            .unwrap_or((None, None))
        {
            (Some(a_far), Some(b_near)) => match delta + b_near - a_far - 1 {
                // Their cells overlap: whatever the contours do inside them,
                // the parts are into each other and the line is not the place
                // to be subtle about it.
                gap if gap < 0 => true,
                // The two frontier cells abut, so the boundary they share is
                // one line of geometry — and only the part of it both actually
                // cover is a seam. A tip that inks its cell without reaching
                // the side of it touches nothing.
                0 => {
                    let ae = lo.facing_cover(horizontal, true, (line - lo.cross) as usize);
                    let be = hi.facing_cover(horizontal, false, (line - hi.cross) as usize);
                    match (ae, be) {
                        (Some(ae), Some(be)) => {
                            covers_meet(ae, lo.profile.edge_den, be, hi.profile.edge_den)
                        }
                        // A profile from before the covers were kept: the cells
                        // are all there is to go on.
                        _ => true,
                    }
                }
                _ => false,
            },
            _ => false,
        };
        run = if touching { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// What `audit max-contact-run` asks of one junction: the run its two parts
/// would share **if they were drawn together until their ink met**, and the
/// cell that costs the layout.
///
/// The run is measured at that meeting and not where the line happens to put
/// the parts, because the demand is a property of the *pair*: a junction that
/// was given its cell has to keep reading as though it had none to spare, or
/// the space the rule asked for would be counted a second time as room the
/// glyph could spend elsewhere. This is exactly how a hardblank behaves — it
/// occupies its cell wherever the parts sit — and the two are the same
/// statement made twice, which is why `owed` nets one against the other: a
/// hardblank already holding the parts apart is the rule's answer, not a second
/// claim on top of it.
///
/// `None` when the two share no line on which both draw ink.
pub fn contact_demand(
    lo: GapSide,
    hi: GapSide,
    horizontal: bool,
    max: u16,
) -> Option<ContactDemand> {
    let facing = facing_offset(lo, hi, horizontal)?;
    // The same measurement with every hardblank stripped off: where the *ink*
    // would meet, which is where the run is counted.
    let ink = lo
        .shared_span(hi, horizontal)
        .filter_map(|line| {
            let a_far = lo.line_at(horizontal, line)?.upper(lo.inner)?.ink()?;
            let b_near = hi.line_at(horizontal, line)?.lower(hi.inner)?.ink()?;
            Some(b_near - a_far - 1)
        })
        .min()?;
    let run = contact_run(lo, hi, horizontal, -ink);
    Some(ContactDemand {
        run,
        // What the hardblanks already hold open, netted off what the rule asks.
        owed: (i32::from(run > max) - (ink - facing)).max(0),
    })
}

/// What [`contact_demand`] found: how far two parts would run together, and
/// what the layout owes them for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactDemand {
    /// The lines their ink would share if they met.
    pub run: u16,
    /// The cells the junction owes on top of what its hardblanks already claim
    /// — 0 or 1, since the rule asks for one cell and asks once.
    pub owed: i32,
}

/// The facing measurement a layout is *scored* on: [`facing_offset`], less what
/// [`contact_demand`] asks of the pair.
///
/// Everything that lays an IDC line out reads this rather than the raw
/// measurement, which is what keeps the rule from being a second kind of
/// number: it is a hardblank the source did not have to write, and it lands in
/// the one place a hardblank would have landed.
pub fn effective_facing(
    lo: GapSide,
    hi: GapSide,
    horizontal: bool,
    max_contact_run: Option<u16>,
) -> Option<i32> {
    let facing = facing_offset(lo, hi, horizontal)?;
    let owed = max_contact_run
        .and_then(|max| contact_demand(lo, hi, horizontal, max))
        .map_or(0, |d| d.owed);
    Some(facing - owed)
}
