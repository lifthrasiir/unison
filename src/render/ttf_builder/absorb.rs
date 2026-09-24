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
//! go. The composite **absorbs** its component: it takes the component's
//! contours moved by its own offset and stops being a composite, and every
//! other `ref` to the component is pointed at the composite instead, with the
//! same offset subtracted. That is the reverse of `inline`, which copies a
//! component into each parent and keeps it only if something else still needs
//! it; here the copy has exactly one home and the parents keep sharing it.
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
//! outline into the host and subtracting the host's offset from every other
//! reference reproduces each glyph's drawing point for point. The host's own
//! traced `contours` are deliberately *not* reused, even though they draw the
//! same thing: they are rounded from pixel space on their own, and a one-unit
//! disagreement with the component would show up in every other parent.

use super::{CollectedGlyph, CompositeRef};
use crate::hash::{HashMap, HashSet};

/// Absorb every component extra that some one-component composite only moves;
/// see the module docs. `extras` names the glyphs the collection added only as
/// components.
pub(super) fn absorb_offset_components(glyphs: &mut Vec<CollectedGlyph>, extras: &HashSet<String>) {
    // component → (host index, the host's offset of it)
    let mut hosts: HashMap<&str, (usize, i16, i16)> = HashMap::default();
    for (i, g) in glyphs.iter().enumerate() {
        if let [cr] = g.composite_refs.as_slice()
            && g.color_layers.is_empty()
            && extras.contains(&cr.component_name)
        {
            hosts
                .entry(cr.component_name.as_str())
                .or_insert((i, cr.x_offset, cr.y_offset));
        }
    }
    if hosts.is_empty() {
        return;
    }
    let hosts: HashMap<String, (usize, i16, i16)> = hosts
        .into_iter()
        .map(|(name, host)| (name.to_string(), host))
        .collect();
    let host_names: HashMap<usize, String> = hosts
        .values()
        .map(|&(host, ..)| (host, glyphs[host].name.clone()))
        .collect();

    // A host is a composite and a component extra is not, so no glyph is on
    // both sides of a move and the moves cannot see each other's results.
    let index: HashMap<&str, usize> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.name.as_str(), i))
        .collect();
    let moves: Vec<(usize, usize, i16, i16)> = hosts
        .iter()
        .map(|(name, &(host, dx, dy))| (index[name.as_str()], host, dx, dy))
        .collect();
    for (component, host, dx, dy) in moves {
        // `collect` synthesizes a component extra without components.
        debug_assert!(glyphs[component].composite_refs.is_empty());
        let moved = glyphs[component]
            .contours
            .iter()
            .map(|c| c.iter().map(|&(x, y)| (x + dx, y + dy)).collect())
            .collect();
        let g = &mut glyphs[host];
        g.contours = moved;
        g.composite_refs.clear();
    }
    for g in glyphs.iter_mut() {
        for cr in &mut g.composite_refs {
            if let Some(&(host, dx, dy)) = hosts.get(&cr.component_name) {
                *cr = CompositeRef {
                    component_name: host_names[&host].clone(),
                    x_offset: cr.x_offset - dx,
                    y_offset: cr.y_offset - dy,
                };
            }
        }
    }
    glyphs.retain(|g| !hosts.contains_key(&g.name));
}
