//! Tests for `meta bitmap-axis`: one font carrying both drawings.
//!
//! The assertions that matter are about the *built font*, not the stage that
//! feeds it — so these load the bytes back with `skrifa` and draw the glyph at
//! each end of the axis, which is what a rasterizer will do.

use super::*;
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};

/// Every point a glyph draws at one axis setting, rounded to whole units and
/// sorted — enough to say "these are the same drawing" without depending on
/// contour order or on where a contour starts.
#[derive(Default)]
struct Points(Vec<(i32, i32)>);

impl OutlinePen for Points {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push((x.round() as i32, y.round() as i32));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push((x.round() as i32, y.round() as i32));
    }
    fn quad_to(&mut self, _: f32, _: f32, x: f32, y: f32) {
        self.0.push((x.round() as i32, y.round() as i32));
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, x: f32, y: f32) {
        self.0.push((x.round() as i32, y.round() as i32));
    }
    fn close(&mut self) {}
}

/// Draw `gid` at `bmap`, in font units, deduplicated and sorted.
fn drawn_at(ttf: &[u8], gid: u16, bmap: f32) -> Vec<(i32, i32)> {
    let font = read_fonts::FontRef::new(ttf).unwrap();
    let axes = font.axes();
    let loc = axes.location([("BMAP", bmap)]);
    let outlines = font.outline_glyphs();
    let glyph = outlines
        .get(skrifa::GlyphId::new(gid as u32))
        .expect("glyph should have an outline");
    let mut pen = Points::default();
    glyph
        .draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::from(&loc)),
            &mut pen,
        )
        .unwrap();
    let mut pts = pen.0;
    pts.sort_unstable();
    pts.dedup();
    pts
}

/// The two drawings of the same source, as the two builds produce them, with
/// the same treatment applied so they can be compared to what the font draws.
fn collected_points(doc: &Document, name: &str, bitmap: bool) -> Vec<(i32, i32)> {
    let (_, _, glyphs, _, _) = collect_glyph_data(&[doc], bitmap).unwrap();
    let g = glyphs.iter().find(|g| g.name == name).unwrap();
    let mut pts: Vec<(i32, i32)> = g
        .contours
        .iter()
        .flatten()
        .map(|&(x, y)| (x as i32, y as i32))
        .collect();
    pts.sort_unstable();
    pts.dedup();
    pts
}

const SOURCE: &str = "\
meta height 4
meta ascent 4
meta descent 0
meta bitmap-axis
glyph slope 2 2
b...
....
map A = slope
";

/// The whole point: one font, and the axis switches between the two drawings.
/// Each end is compared against what that build actually produced, so this
/// fails if the padding, the deltas or the tent is wrong.
#[test]
fn the_axis_switches_between_the_two_drawings() {
    let doc = document_io::parse_document_from_str(SOURCE, "test.unf".into()).unwrap();
    let ttf = build_font_from_documents(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    let gid = font.cmap().unwrap().map_codepoint('A').unwrap().to_u32() as u16;

    assert_eq!(
        drawn_at(&ttf, gid, 0.0),
        collected_points(&doc, "slope", false),
        "at BMAP=0 the font must draw the vector master",
    );
    assert_eq!(
        drawn_at(&ttf, gid, 1.0),
        collected_points(&doc, "slope", true),
        "at BMAP=1 the font must draw the bitmap master",
    );
    assert_ne!(
        drawn_at(&ttf, gid, 0.0),
        drawn_at(&ttf, gid, 1.0),
        "the fixture is pointless if the two ends agree",
    );
}

/// The tent is what makes the axis a switch rather than a slider: below its
/// start nothing has moved yet. A caller setting 0.5 gets the vector drawing,
/// not a half-interpolated nonsense.
#[test]
fn the_tent_keeps_every_reachable_value_on_one_master_or_the_other() {
    let doc = document_io::parse_document_from_str(SOURCE, "test.unf".into()).unwrap();
    let ttf = build_font_from_documents(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    let gid = font.cmap().unwrap().map_codepoint('A').unwrap().to_u32() as u16;

    let vector = drawn_at(&ttf, gid, 0.0);
    for v in [0.1f32, 0.25, 0.5, 0.75, 0.9, 0.98] {
        assert_eq!(
            drawn_at(&ttf, gid, v),
            vector,
            "BMAP={v} should still be the vector drawing",
        );
    }
}

/// Without the `meta` key there is no axis and no variation data at all — the
/// font is exactly the static one it always was.
#[test]
fn without_the_meta_key_nothing_variable_is_emitted() {
    let plain = SOURCE.replace("meta bitmap-axis\n", "");
    let doc = document_io::parse_document_from_str(&plain, "test.unf".into()).unwrap();
    let ttf = build_font_from_documents(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    assert!(font.fvar().is_err(), "no fvar without the key");
    assert!(font.gvar().is_err(), "no gvar without the key");
    assert!(font.stat().is_err(), "no STAT without the key");
}

/// The axis is labelled by a name record of its own, defaulted when the source
/// says nothing and overridable when it does.
#[test]
fn the_axis_name_is_declared_or_defaulted() {
    for (source, want) in [
        (SOURCE.to_string(), "Bitmap"),
        (
            SOURCE.replace(
                "meta bitmap-axis\n",
                "meta bitmap-axis\nmeta bitmap-axis-name Pixels\n",
            ),
            "Pixels",
        ),
    ] {
        let doc = document_io::parse_document_from_str(&source, "test.unf".into()).unwrap();
        let ttf = build_font_from_documents(&[&doc]).expect("font should build");
        let font = read_fonts::FontRef::new(&ttf).unwrap();
        let axis = font.axes().iter().next().expect("one axis");
        assert_eq!(axis.tag().to_string(), "BMAP");
        let name_id = axis.name_id();
        let got = font
            .localized_strings(name_id)
            .next()
            .expect("the axis name record must exist")
            .to_string();
        assert_eq!(got, want);
    }
}

/// `gvar` is indexed by glyph id, so it must carry an entry for *every* glyph
/// the font ends up with — including the ones `add_color_layer_glyphs`
/// synthesizes after the glyph list is collected. A `gvar` shorter than
/// `maxp.numGlyphs` is not a partial font: Firefox's sanitiser rejects the
/// table and drops **all** variation data, so the axis silently stops working
/// while Chrome carries on regardless.
#[test]
fn gvar_covers_every_glyph_including_the_synthesized_colour_layers() {
    let input = "\
meta height 4
meta ascent 4
meta descent 0
meta bitmap-axis
color red = #FF0000
color blue = #0000FF
glyph base 2 2
b...
....
glyph overlay 2 2
..@@
@@..
glyph combo
ref base fill red
ref overlay fill blue
map A = combo
map B = base
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let ttf = build_font_from_documents(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    let num_glyphs = font.maxp().unwrap().num_glyphs();
    let gvar = font.gvar().expect("the font should carry gvar");
    assert!(
        font.colr().is_ok(),
        "the fixture is pointless without colour layers to synthesize glyphs for",
    );
    assert_eq!(
        gvar.glyph_count(),
        num_glyphs,
        "gvar must have an entry per glyph, colour layer glyphs included",
    );
}

/// The COLR layer glyphs of `name`'s base glyph, as GIDs in layer order.
fn layer_gids(ttf: &[u8], ch: char) -> Vec<u16> {
    let font = read_fonts::FontRef::new(ttf).unwrap();
    let base_gid = font.cmap().unwrap().map_codepoint(ch).unwrap().to_u32() as u16;
    let colr = font.colr().expect("the fixture should build a COLR table");
    let bases = colr
        .base_glyph_records()
        .expect("COLRv0 base glyph records")
        .unwrap();
    let layers = colr.layer_records().expect("COLRv0 layer records").unwrap();
    let base = bases
        .iter()
        .find(|b| b.glyph_id().to_u32() as u16 == base_gid)
        .expect("the mapped glyph should be a COLR base glyph");
    let first = base.first_layer_index() as usize;
    layers[first..first + base.num_layers() as usize]
        .iter()
        .map(|l| l.glyph_id().to_u32() as u16)
        .collect()
}

/// One color layer's points, as the given build produced them.
fn collected_layer_points(doc: &Document, name: &str, layer: usize, bitmap: bool) -> Vec<(i32, i32)> {
    let (_, _, glyphs, _, _) = collect_glyph_data(&[doc], bitmap).unwrap();
    let g = glyphs.iter().find(|g| g.name == name).unwrap();
    let mut pts: Vec<(i32, i32)> = g.color_layers[layer]
        .contours
        .iter()
        .flatten()
        .map(|&(x, y)| (x as i32, y as i32))
        .collect();
    pts.sort_unstable();
    pts.dedup();
    pts
}

const COLOR_SOURCE: &str = "\
meta height 4
meta ascent 4
meta descent 0
meta bitmap-axis
color red = #FF0000
glyph slope 2 2
b...
....
glyph tint 2 2
ref slope fill red
map A = tint
";

/// A colour glyph's layers are outlines like any other, so the axis has to
/// switch them too: the drawing a COLR layer glyph carries is what actually
/// reaches the screen for an emoji, and freezing it at the vector master left
/// the bitmap face drawing sub-pixel shapes.
#[test]
fn a_colour_layer_follows_the_axis() {
    let doc = document_io::parse_document_from_str(COLOR_SOURCE, "test.unf".into()).unwrap();
    let ttf = build_font_from_documents(&[&doc]).expect("font should build");
    let gids = layer_gids(&ttf, 'A');
    assert_eq!(gids.len(), 1, "the fixture has one colour layer");

    assert_eq!(
        drawn_at(&ttf, gids[0], 0.0),
        collected_layer_points(&doc, "tint", 0, false),
        "at BMAP=0 a colour layer must draw the vector master",
    );
    assert_eq!(
        drawn_at(&ttf, gids[0], 1.0),
        collected_layer_points(&doc, "tint", 0, true),
        "at BMAP=1 a colour layer must draw the bitmap master",
    );
    assert_ne!(
        drawn_at(&ttf, gids[0], 0.0),
        drawn_at(&ttf, gids[0], 1.0),
        "the fixture is pointless if the two ends agree",
    );
}

/// A layer the bitmap build lights no pixel of (`:zero`) is dropped from *its*
/// layer list, so the two lists no longer line up by position. Matching them by
/// position would then vary one layer into another's shape — hence the source
/// identity every layer carries. The dropped layer itself collapses, which is
/// the only way an outline can say "not drawn at this end of the axis".
#[test]
fn a_layer_only_the_vector_build_draws_collapses_and_misplaces_nobody() {
    let input = "\
meta height 4
meta ascent 4
meta descent 0
meta bitmap-axis
color red = #FF0000
color blue = #0000FF
glyph slope 2 2
b...
....
glyph tint 2 2
ref 2x2-circle:zero fill red
ref slope fill blue
map A = tint
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let ttf = build_font_from_documents(&[&doc]).expect("font should build");
    let gids = layer_gids(&ttf, 'A');
    assert_eq!(gids.len(), 2, "the vector build keeps both layers");
    assert_eq!(
        collect_glyph_data(&[&doc], true).unwrap().2
            .iter()
            .find(|g| g.name == "tint")
            .unwrap()
            .color_layers
            .len(),
        1,
        "the fixture is pointless unless the bitmap build drops the `:zero` layer",
    );

    let collapsed = drawn_at(&ttf, gids[0], 1.0);
    assert!(
        collapsed.len() <= 1,
        "a layer the bitmap build does not draw must collapse to a point, got {collapsed:?}",
    );
    assert!(
        drawn_at(&ttf, gids[0], 0.0).len() > 1,
        "the same layer must still be drawn at the vector end",
    );
    assert_eq!(
        drawn_at(&ttf, gids[1], 1.0),
        collected_layer_points(&doc, "tint", 0, true),
        "the surviving layer must vary into its own bitmap drawing, not another's",
    );
}
