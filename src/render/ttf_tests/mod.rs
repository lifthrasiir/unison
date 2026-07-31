//! Tests for the TrueType builder.
//!
//! Declared as a child module of [`super::ttf_builder`] through `#[path]`, so
//! these still reach that module's private items. This file holds the shared
//! helpers; the tests themselves are grouped by subject in the submodules.

use super::*;
use super::contours::layers_have_subpixel_conflicts;
use super::gpos::build_anchor_gpos;
use super::gsub::build_gsub;
use super::hints::generate_grid_snap_hints;
use super::tables::glyph_bounds;
use read_fonts::TableProvider;

mod color;
mod composite;
mod gpos;
mod gsub;
mod hints;
mod misc;

/// Rotate a contour so it starts at its lexicographically-smallest point.
/// `render::contour::track_contour` traces contours via `HashMap`
/// iteration order internally (not in this file's scope to change), so
/// the *rotation* at which a closed contour's point list starts (and the
/// relative order of multiple disjoint sub-contours within one glyph)
/// varies nondeterministically from run to run, even though the actual
/// traced geometry does not. Canonicalizing before hashing makes the
/// digest reflect real geometry changes only.
/// Drop vertices that sit exactly on the straight line between their
/// neighbors (cyclically). `track_contour`'s collinearity-collapsing
/// depends on which point tracing happened to start at (see
/// `canonicalize_contour` doc comment), so which redundant on-line
/// points survive is itself nondeterministic; simplifying away all of
/// them makes the polygon's *point set* canonical, not just its
/// rotation.
fn simplify_collinear(c: &[(i16, i16)]) -> Vec<(i16, i16)> {
    let n = c.len();
    if n < 3 {
        return c.to_vec();
    }
    (0..n)
        .filter(|&i| {
            let (x1, y1) = c[(i + n - 1) % n];
            let (x2, y2) = c[i];
            let (x3, y3) = c[(i + 1) % n];
            let cross = (x2 - x1) as i64 * (y3 - y1) as i64 - (y2 - y1) as i64 * (x3 - x1) as i64;
            cross != 0
        })
        .map(|i| c[i])
        .collect()
}

fn canonicalize_contour(c: &[(i16, i16)]) -> Vec<(i16, i16)> {
    let c = simplify_collinear(c);
    if c.is_empty() {
        return Vec::new();
    }
    let min_idx = c
        .iter()
        .enumerate()
        .min_by_key(|&(_, &pt)| pt)
        .map(|(i, _)| i)
        .unwrap();
    let mut rotated = Vec::with_capacity(c.len());
    rotated.extend_from_slice(&c[min_idx..]);
    rotated.extend_from_slice(&c[..min_idx]);
    rotated
}

fn canonicalize_glyph(g: &CollectedGlyph) -> (Vec<u32>, u16, Vec<Vec<(i16, i16)>>) {
    let mut contours: Vec<Vec<(i16, i16)>> =
        g.contours.iter().map(|c| canonicalize_contour(c)).collect();
    contours.sort();
    (g.codepoints.clone(), g.advance_width, contours)
}

/// Nonzero winding number of `contours` at `(x, y)`, in font units.
fn winding_at(contours: &[Vec<(i16, i16)>], x: f32, y: f32) -> i32 {
    let mut winding = 0;
    for contour in contours {
        for i in 0..contour.len() {
            let (x0, y0) = contour[i];
            let (x1, y1) = contour[(i + 1) % contour.len()];
            let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
            if y0 <= y {
                if y1 > y && (x1 - x0) * (y - y0) - (x - x0) * (y1 - y0) > 0.0 {
                    winding += 1;
                }
            } else if y1 <= y && (x1 - x0) * (y - y0) - (x - x0) * (y1 - y0) < 0.0 {
                winding -= 1;
            }
        }
    }
    winding
}

/// Build `input` into a font and shape `text` through it, returning the
/// resulting glyph names. Table-level assertions cannot tell "emitted" from
/// "emitted and actually reachable by a shaper" — and every GSUB bug found
/// so far lived exactly in that gap.
fn shape_glyph_names(input: &str, text: &str) -> Vec<String> {
    shape_glyph_names_in(input, text, None)
}

/// As [`shape_glyph_names`], but shaping under a BCP 47 language, which is
/// what makes a shaper look for an explicit LangSys record instead of the
/// script's default one.
fn shape_glyph_names_in(input: &str, text: &str, language: Option<&str>) -> Vec<String> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let built = build_font_with_gid_map(&[&doc]).expect("font should build");
    let face = rustybuzz::Face::from_slice(&built.ttf, 0).expect("font should parse");
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    if let Some(language) = language {
        buffer.set_language(language.parse().expect("valid BCP 47 language"));
    }
    let out = rustybuzz::shape(&face, &[], buffer);
    out.glyph_infos()
        .iter()
        .map(|i| {
            built
                .gid_to_name
                .get(&(i.glyph_id as u16))
                .cloned()
                .unwrap_or_else(|| format!("gid{}", i.glyph_id))
        })
        .collect()
}

/// `maxp` limits recomputed from the emitted `glyf`, the way OTS and
/// friends check them: point/contour counts of composites are the *fully
/// decomposed* totals, and COLR layer glyphs count like any other outline.
fn recomputed_maxp(bytes: &[u8]) -> HashMap<&'static str, u16> {
    let font = read_fonts::FontRef::new(bytes).unwrap();
    let glyf = font.glyf().unwrap();
    let loca = font.loca(None).unwrap();
    let num_glyphs = font.maxp().unwrap().num_glyphs();

    // (points, contours, depth) of a glyph after full decomposition.
    fn stats(
        gid: GlyphId,
        glyf: &read_fonts::tables::glyf::Glyf,
        loca: &read_fonts::tables::loca::Loca,
        seen: &mut Vec<u32>,
    ) -> (u32, u32, u16) {
        if seen.contains(&gid.to_u32()) {
            return (0, 0, 0); // cycle guard; must not happen
        }
        match loca.get_glyf(gid, glyf).unwrap() {
            None => (0, 0, 0),
            Some(read_fonts::tables::glyf::Glyph::Simple(s)) => {
                (s.num_points() as u32, s.end_pts_of_contours().len() as u32, 0)
            }
            Some(read_fonts::tables::glyf::Glyph::Composite(c)) => {
                seen.push(gid.to_u32());
                let (mut p, mut n, mut d) = (0, 0, 0);
                for comp in c.components() {
                    let (cp, cn, cd) = stats(GlyphId::from(comp.glyph), glyf, loca, seen);
                    p += cp;
                    n += cn;
                    d = d.max(cd + 1);
                }
                seen.pop();
                (p, n, d)
            }
        }
    }

    let mut m: HashMap<&'static str, u16> = HashMap::new();
    for raw in 0..num_glyphs as u32 {
        let gid = GlyphId::new(raw);
        let glyph = loca.get_glyf(gid, &glyf).unwrap();
        let (p, n, d) = stats(gid, &glyf, &loca, &mut Vec::new());
        let (points, contours) = match glyph {
            Some(read_fonts::tables::glyf::Glyph::Composite(c)) => {
                let elems = c.components().count() as u16;
                let e = m.entry("maxComponentElements").or_insert(0);
                *e = (*e).max(elems);
                let e = m.entry("maxComponentDepth").or_insert(0);
                *e = (*e).max(d);
                ("maxCompositePoints", "maxCompositeContours")
            }
            Some(read_fonts::tables::glyf::Glyph::Simple(_)) => ("maxPoints", "maxContours"),
            None => continue,
        };
        let e = m.entry(points).or_insert(0);
        *e = (*e).max(p as u16);
        let e = m.entry(contours).or_insert(0);
        *e = (*e).max(n as u16);
    }
    m
}
