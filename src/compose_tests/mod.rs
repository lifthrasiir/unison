//! Tests for the IDC line and the variant name rule.

use super::enclosure::cavity_fits;
use super::gap::{ContactDemand, contact_demand, contact_run, facing_offset};
use super::ink::{InkLine, WallFace};
use super::*;
use crate::document::{ComposeItem, GlyphCompose, GlyphRef, Margin, PixelGrid};
use crate::issues::Severity;

/// One `audit ideal-clearance` band, as a test writes it: the same range for a
/// split and for an enclosure, which is what a source stating one pair means.
pub(super) fn band(min: i16, max: i16) -> crate::audit::ClearanceBand {
    crate::audit::ClearanceBand {
        linear: (min, max),
        enclosing: (min, max),
    }
}

/// One line of a part that does not enclose: a single run, whose inner faces
/// are its outer ones crossed over. Every profile a one-dimensional test
/// builds is made of these.
pub(super) fn one_run(near: i32, far: i32, near_hardblanks: u16, far_hardblanks: u16) -> InkLine {
    // A pivot the run covers, so the one run is the wall on both sides — which
    // is what a line of a part spanning the box is.
    InkLine::from_runs(&[(near, far, near_hardblanks, far_hardblanks)], near)
        .expect("one run is a line")
}

/// [`facing_offset`] between two parts of a one-dimensional line.
pub(super) fn facing(a: &InkProfile, b: &InkProfile, horizontal: bool) -> Option<i32> {
    facing_offset(GapSide::linear(a), GapSide::linear(b), horizontal)
}

/// [`contact_run`] between two parts of a one-dimensional line.
pub(super) fn contact(a: &InkProfile, b: &InkProfile, horizontal: bool, delta: i32) -> u16 {
    contact_run(GapSide::linear(a), GapSide::linear(b), horizontal, delta)
}

/// [`contact_demand`] between two parts of a one-dimensional line.
pub(super) fn demand(
    a: &InkProfile,
    b: &InkProfile,
    horizontal: bool,
    max: u16,
) -> Option<ContactDemand> {
    contact_demand(GapSide::linear(a), GapSide::linear(b), horizontal, max)
}

/// A parent with no `scale` and no `origin`.
pub(super) const UNIT: Raster = Raster {
    scale: 1,
    origin: (0, 0),
};

pub(super) fn scaled(scale: u8) -> Raster {
    Raster {
        scale,
        origin: (0, 0),
    }
}

pub(super) fn part(name: &str) -> ComposeItem {
    ComposeItem::Part {
        name: name.to_string(),
        raw_name: None,
    }
}

/// A part written as `raw` whose name resolves, through an alias, to `name`.
pub(super) fn aliased(raw: &str, name: &str) -> ComposeItem {
    ComposeItem::Part {
        name: name.to_string(),
        raw_name: Some(raw.to_string()),
    }
}

pub(super) fn line(op: IdcOp, items: Vec<ComposeItem>) -> GlyphCompose {
    GlyphCompose {
        op,
        items,
        assumed: false,
        comment: None,
    }
}

/// `dims` over a table, with everything else unknown.
pub(super) fn table<'a>(entries: &'a [(&'a str, (u16, u16))]) -> impl Fn(&str) -> PartDims + 'a {
    move |name: &str| {
        entries
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(PartDims::Unknown, |(_, (w, h))| {
                PartDims::Size(*w, *h, Margin::default())
            })
    }
}

pub(super) fn expand(
    parent: Option<(u16, u16)>,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
) -> (Vec<GlyphRef>, Vec<(Severity, String)>) {
    expand_compose("test", parent, UNIT, compose, dims, None, None)
}

pub(super) fn of_severity(issues: &[(Severity, String)], want: Severity) -> Vec<&str> {
    issues
        .iter()
        .filter(|(s, _)| *s == want)
        .map(|(_, m)| m.as_str())
        .collect()
}

pub(super) fn errors(issues: &[(Severity, String)]) -> Vec<&str> {
    of_severity(issues, Severity::Error)
}

pub(super) fn todos(issues: &[(Severity, String)]) -> Vec<&str> {
    of_severity(issues, Severity::Todo)
}

/// A grid from a picture: `#` is ink, `$` a hardblank, anything else nothing.
///
/// Three sub-pixel shapes have a character too, for the cells whose *contours*
/// decide a contact: `/` covers none of its right edge, `^` the top half of it,
/// and `v` the bottom half of its left edge.
pub(super) fn grid(rows: &[&str]) -> PixelGrid {
    let mut g = PixelGrid::new(rows[0].len() as u16, rows.len() as u16);
    for (r, row) in rows.iter().enumerate() {
        for (c, ch) in row.chars().enumerate() {
            let shape = match ch {
                '#' => crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
                '$' => crate::pixel::PixelShape::new(crate::pixel::PX_HARDBLANK, false),
                '/' => crate::pixel::PixelShape::new(crate::pixel::PX_HALF1, true),
                '^' => crate::pixel::PixelShape::new(crate::pixel::PX_CORNER2, true),
                'v' => crate::pixel::PixelShape::new(crate::pixel::PX_CORNER1, true),
                _ => continue,
            };
            g.set(r as u16, c as u16, shape);
        }
    }
    g
}

/// The profile of a grid that is its own declared box, which is what a part
/// with no `origin`/`extent` of its own is.
pub(super) fn whole(g: &PixelGrid, scale: u8) -> InkProfile {
    let s = scale.max(1) as u16;
    InkProfile::of(g, scale, (0, 0), (0, 0), (g.width / s, g.height / s))
}

pub(super) fn profiles(entries: &[(&str, &[&str])]) -> crate::hash::HashMap<String, InkProfile> {
    entries
        .iter()
        .map(|(name, rows)| (name.to_string(), whole(&grid(rows), 1)))
        .collect()
}

mod clearance;
mod enclosures;
mod pipeline;
mod splits;
mod variants;
