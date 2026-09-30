//! Which variation sequences draw exactly what their base character does —
//! what [`super::SpecimenOptions::hide_same_as_base`] leaves out.
//!
//! Asked of the *built fonts*, not of the source, because a name tells too
//! little: `glyph han-4e00.0:* = han-4e00:*` makes every sized variant of the
//! two one glyph while `han-4e00` and `han-4e00.0` themselves stay two, each a
//! glyph of its own that happens to draw the same thing. So two glyph ids are
//! the same glyph here when both fonts of the pair draw them alike: the same
//! advance and, point for point, the same outline. A colour glyph draws its
//! layers rather than its outline, and comparing those is more than this is
//! worth, so one is the same only as its own glyph id.
//!
//! Computed where the font bytes are made (the background rebuild, see
//! `app/background.rs`), because it walks every sequence the font has.

use crate::hash::HashSet;
use read_fonts::TableProvider;
use skrifa::charmap::MapVariant;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Every `(base, selector)` pair that draws the same as `base` alone in every
/// font of `fonts`. A font that does not parse counts as no font at all, and a
/// pair the first font does not list is not asked about.
pub fn same_as_base(fonts: &[&[u8]]) -> HashSet<(u32, u32)> {
    let fonts: Vec<Face> = fonts
        .iter()
        .filter_map(|b| FontRef::new(b).ok())
        .map(|font| Face {
            charmap: font.charmap(),
            metrics: font.glyph_metrics(Size::unscaled(), LocationRef::default()),
            outlines: font.outline_glyphs(),
            colr: font.colr().ok(),
        })
        .collect();
    let Some(first) = fonts.first() else {
        return HashSet::default();
    };
    first
        .charmap
        .variant_mappings()
        .map(|(base, selector, _)| (base, selector))
        .filter(|&(base, selector)| fonts.iter().all(|f| f.draws_same(base, selector)))
        .collect()
}

/// What of one font the comparison reads, set up once rather than per pair.
struct Face<'a> {
    charmap: skrifa::charmap::Charmap<'a>,
    metrics: skrifa::metrics::GlyphMetrics<'a>,
    outlines: skrifa::outline::OutlineGlyphCollection<'a>,
    colr: Option<read_fonts::tables::colr::Colr<'a>>,
}

impl Face<'_> {
    fn draws_same(&self, base: u32, selector: u32) -> bool {
        let Some(base_gid) = self.charmap.map(base) else {
            return false;
        };
        let gid = match self.charmap.map_variant(base, selector) {
            Some(MapVariant::UseDefault) => return true,
            Some(MapVariant::Variant(gid)) => gid,
            None => return false,
        };
        if gid == base_gid {
            return true;
        }
        if self.is_color(gid) || self.is_color(base_gid) {
            return false;
        }
        if self.metrics.advance_width(gid) != self.metrics.advance_width(base_gid) {
            return false;
        }
        match (self.outline(gid), self.outline(base_gid)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    fn is_color(&self, gid: GlyphId) -> bool {
        self.colr.as_ref().is_some_and(|colr| {
            colr.v0_base_glyph(gid).is_ok_and(|r| r.is_some())
                || colr.v1_base_glyph(gid).is_ok_and(|r| r.is_some())
        })
    }

    /// The outline as the pen sees it, in font units at the default location.
    /// Exact `f32` comparison is what is wanted: both sides come off the same
    /// integer coordinates, so the same drawing is bit for bit the same list.
    fn outline(&self, gid: GlyphId) -> Option<Vec<Segment>> {
        let glyph = self.outlines.get(gid)?;
        let mut pen = Recorder(Vec::new());
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .ok()?;
        Some(pen.0)
    }
}

#[derive(PartialEq)]
enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Curve(f32, f32, f32, f32, f32, f32),
    Close,
}

struct Recorder(Vec<Segment>);

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Segment::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(Segment::Line(x, y));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.push(Segment::Quad(cx0, cy0, x, y));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(Segment::Curve(cx0, cy0, cx1, cy1, x, y));
    }
    fn close(&mut self) {
        self.0.push(Segment::Close);
    }
}
