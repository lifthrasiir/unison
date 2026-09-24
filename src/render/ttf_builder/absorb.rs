//! Folding a component glyph into the one composite that only moves it.
//!
//! Every Han character in `font/` is drawn in a box of its own
//! (`han-XXXX:15x16`) that nothing maps, and reaches the cmap through a glyph
//! that is a single `ref` to that drawing at an offset (`han.unf`'s
//! `glyph han-($1) … ref ($0) 1 0`). Emitted as written that is two glyph ids
//! per character — a simple glyph nobody maps and a one-component composite —
//! and with the Han repertoire the second id was over a quarter of the font's
//! 65,535.
//!
//! The two ids draw the same outline a translation apart, so one of them can
//! go. The composite **absorbs** its component: it takes over what the
//! component is made of — its contours, or its own components — moved by its
//! own offset, and every other `ref` to the component is pointed at the
//! composite instead, with the same offset subtracted. That is the reverse of
//! `inline`, which copies a component into each parent and keeps it only if
//! something else still needs it; here the copy has exactly one home and the
//! parents keep sharing it.
//!
//! A component is itself kept as a composite where its source is one (see the
//! component loop in [`super::collect`]), so a part drawn once and placed by
//! many characters is stored once. That makes chains: `han-XXXX` moves
//! `han-XXXX:15x16`, which may be nothing but a `ref` to another drawing
//! again. A glyph is never host and absorbed in the same pass — the moves of
//! one pass could otherwise see each other's half-done results — so the pass
//! runs until nothing moves, one link of a chain at a time.
//!
//! # Dissolving what a glyph id buys nothing for
//!
//! Once the chains are absorbed, a component extra is worth an id of its own
//! only as a *drawing several glyphs share*. Two kinds are not, and are
//! dissolved into their users:
//!
//! - **A composite part**, whatever its number of users. It is nothing but
//!   component records, so splicing those into each user (offsets added) costs
//!   a few bytes a record and draws the same thing.
//! - **A drawing exactly one `ref` in the font points at.** Its user becomes a
//!   simple glyph drawing its own traced outline — a TrueType glyph cannot be
//!   part contours and part components — which costs that user the sharing of
//!   its other components; the id matters more.
//!
//! Over `font/` the ids a shared drawing still keeps are about 1,300 of them
//! (a drawing used by two or more glyphs), against some 2 MB the sharing saves.
//!
//! Absorbing and dissolving leave components unreferenced, and so does the
//! colour pass: a coloured composite is flattened into layers and drops its
//! components. So every round starts by dropping those.
//!
//! # What may be absorbed
//!
//! Only a glyph the collection added *because* a composite referenced it — a
//! component extra, see [`super::collect`]. Anything else is reachable by its
//! own glyph id: a cmap entry, a `remap`, an anchor alternative, `keep`. Such a
//! glyph has to stay under its own name, and every name GSUB and GPOS resolve
//! is exactly one of those, so nothing downstream looks the dropped name up.
//!
//! The absorbing glyph is the first one-component composite of it in glyph
//! order, which is deterministic and face-independent (see the sort in
//! [`super::collect`]), so a collection's faces and both builds of
//! `meta bitmap-axis` still agree on glyph ids and on every composite's
//! components — [`super::masters::assert_composites_agree`] depends on that.
//! It is decided from the `ref` structure alone and never from the outline for
//! the same reason: the two builds may draw a component differently, but they
//! place it the same way.
//!
//! # Why it is exact
//!
//! Everything here is already in font units. A [`CompositeRef`] offset is an
//! integer translation of the component's own integer outline, so moving the
//! outline (or the component list) into the host and subtracting the host's
//! offset from every other reference reproduces each glyph's drawing point for
//! point. The host's own traced `contours` are deliberately *not* reused, even
//! though they draw the same thing: they are rounded from pixel space on their
//! own, and a one-unit disagreement with the component would show up in every
//! other parent.
//!
//! # Why it works on indices
//!
//! Every round rereads the whole `ref` graph, and the rounds run until nothing
//! moves. Keyed by name, each round rebuilt maps over some 50,000 strings, and
//! the stage cost more than a tenth of the collection it follows. So the graph
//! is read into glyph indices once, rewritten there, and written back once.

use super::{CollectedGlyph, CompositeRef};
use crate::hash::HashMap;

/// One component reference, by glyph index.
#[derive(Clone, Copy)]
struct Ref {
    to: usize,
    dx: i16,
    dy: i16,
}

/// The `ref` graph of the collected glyphs, by index.
struct Graph {
    refs: Vec<Vec<Ref>>,
    alive: Vec<bool>,
    /// Added only as a component: the glyphs that may be absorbed or dissolved.
    extra: Vec<bool>,
    /// Flattened into colour layers: never a host, never absorbed.
    colored: Vec<bool>,
    /// Whose `refs` no longer match its `composite_refs`.
    dirty: Vec<bool>,
}

/// Absorb every component extra that some one-component composite only moves,
/// dissolve the ones that buy no sharing, and drop the ones nothing refers to;
/// see the module docs. The component extras are `glyphs[first_extra..]`.
pub(super) fn absorb_offset_components(glyphs: &mut Vec<CollectedGlyph>, first_extra: usize) {
    let index: HashMap<&str, usize> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.as_str(), i))
        .collect();
    // A glyph naming a component that is not collected keeps its references
    // exactly as written and takes no part. The component loop in `collect`
    // collects every name a reference makes, so this is a guard, not a case.
    let mut frozen = vec![false; glyphs.len()];
    let refs: Vec<Vec<Ref>> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let refs: Option<Vec<Ref>> = g
                .composite_refs
                .iter()
                .map(|cr| {
                    let &to = index.get(cr.component_name.as_str())?;
                    Some(Ref {
                        to,
                        dx: cr.x_offset,
                        dy: cr.y_offset,
                    })
                })
                .collect();
            refs.unwrap_or_else(|| {
                debug_assert!(false, "{} names an uncollected component", g.name);
                frozen[i] = true;
                Vec::new()
            })
        })
        .collect();
    drop(index);
    let n = glyphs.len();
    let mut graph = Graph {
        refs,
        alive: vec![true; n],
        extra: (0..n).map(|i| i >= first_extra && !frozen[i]).collect(),
        colored: glyphs.iter().map(|g| !g.color_layers.is_empty()).collect(),
        dirty: vec![false; n],
    };
    // A frozen glyph's own references are not in the graph, so what it names
    // must not be taken away from under it.
    for (i, g) in glyphs.iter().enumerate().filter(|&(i, _)| frozen[i]) {
        graph.colored[i] = true;
        for cr in &g.composite_refs {
            if let Some(j) = glyphs.iter().position(|h| h.name == cr.component_name) {
                graph.extra[j] = false;
            }
        }
    }

    while absorb_once(&mut graph, glyphs) {}
    loop {
        drop_unreferenced(&mut graph);
        if !dissolve_once(&mut graph) {
            break;
        }
    }

    for i in (0..n).filter(|&i| graph.alive[i] && graph.dirty[i]) {
        let refs = graph.refs[i]
            .iter()
            .map(|r| CompositeRef {
                component_name: glyphs[r.to].name.clone(),
                x_offset: r.dx,
                y_offset: r.dy,
            })
            .collect();
        glyphs[i].composite_refs = refs;
    }
    let mut alive = graph.alive.into_iter();
    glyphs.retain(|_| alive.next().unwrap_or(true));
}

/// One pass of absorbing; `false` when nothing moved.
fn absorb_once(graph: &mut Graph, glyphs: &mut [CollectedGlyph]) -> bool {
    let n = graph.refs.len();
    // component → (host, the host's offset of it), the first host in glyph
    // order winning.
    let mut host: Vec<Option<Ref>> = vec![None; n];
    for i in 0..n {
        if let [r] = graph.refs[i][..]
            && graph.alive[i]
            && !graph.colored[i]
            && graph.extra[r.to]
            && !graph.colored[r.to]
            && host[r.to].is_none()
        {
            host[r.to] = Some(Ref { to: i, ..r });
        }
    }
    // A host that is itself being absorbed waits for the next pass.
    for c in 0..n {
        if let Some(h) = host[c]
            && host[h.to].is_some()
        {
            host[c] = None;
        }
    }
    if host.iter().all(Option::is_none) {
        return false;
    }
    for c in 0..n {
        let Some(h) = host[c] else { continue };
        let mut contours = std::mem::take(&mut glyphs[c].contours);
        for p in contours.iter_mut().flatten() {
            *p = (p.0 + h.dx, p.1 + h.dy);
        }
        glyphs[h.to].contours = contours;
        let refs = std::mem::take(&mut graph.refs[c])
            .into_iter()
            .map(|r| Ref {
                dx: r.dx + h.dx,
                dy: r.dy + h.dy,
                ..r
            })
            .collect();
        graph.refs[h.to] = refs;
        graph.dirty[h.to] = true;
        graph.alive[c] = false;
    }
    for i in 0..n {
        for r in graph.refs[i].iter_mut() {
            if let Some(h) = host[r.to] {
                *r = Ref {
                    to: h.to,
                    dx: r.dx - h.dx,
                    dy: r.dy - h.dy,
                };
                graph.dirty[i] = true;
            }
        }
    }
    true
}

/// How many references each glyph receives from the glyphs still alive.
fn uses(graph: &Graph) -> Vec<u32> {
    let mut uses = vec![0u32; graph.refs.len()];
    for (i, refs) in graph.refs.iter().enumerate() {
        if graph.alive[i] {
            for r in refs {
                uses[r.to] += 1;
            }
        }
    }
    uses
}

/// Drop every component extra no glyph refers to, down the whole chain.
fn drop_unreferenced(graph: &mut Graph) {
    let mut uses = uses(graph);
    let mut dead: Vec<usize> = (0..uses.len())
        .filter(|&i| graph.alive[i] && graph.extra[i] && uses[i] == 0)
        .collect();
    while let Some(i) = dead.pop() {
        graph.alive[i] = false;
        for r in &graph.refs[i] {
            uses[r.to] -= 1;
            if uses[r.to] == 0 && graph.alive[r.to] && graph.extra[r.to] {
                dead.push(r.to);
            }
        }
    }
}

/// One pass dissolving component extras into the glyphs that use them;
/// `false` when there was none. See the module docs.
fn dissolve_once(graph: &mut Graph) -> bool {
    let uses = uses(graph);
    // A composite part is nothing but component records, so it is spliced
    // into every user however many there are; a drawing is dissolved only
    // where it has a single user.
    let dissolved: Vec<bool> = (0..uses.len())
        .map(|i| {
            graph.alive[i]
                && graph.extra[i]
                && !graph.colored[i]
                && uses[i] > 0
                && (uses[i] == 1 || !graph.refs[i].is_empty())
        })
        .collect();
    let mut changed = false;
    for i in 0..uses.len() {
        // A glyph that is being dissolved itself waits for the next pass, so
        // nothing it hands over is half-rewritten.
        if !graph.alive[i] || dissolved[i] || !graph.refs[i].iter().any(|r| dissolved[r.to]) {
            continue;
        }
        let mut refs = Vec::new();
        let mut flatten = false;
        for r in &graph.refs[i] {
            if !dissolved[r.to] {
                refs.push(*r);
            } else if graph.refs[r.to].is_empty() {
                flatten = true;
                break;
            } else {
                refs.extend(graph.refs[r.to].iter().map(|inner| Ref {
                    to: inner.to,
                    dx: inner.dx + r.dx,
                    dy: inner.dy + r.dy,
                }));
            }
        }
        // Flattened, the glyph draws its own traced outline, which every
        // composite carries beside its components.
        graph.refs[i] = if flatten { Vec::new() } else { refs };
        graph.dirty[i] = true;
        changed = true;
    }
    changed
}
