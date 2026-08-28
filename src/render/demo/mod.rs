//! `demo.html`: the font's one self-contained demonstration page.
//!
//! It is what `sample.html`, `sample.png` and `live.html` grew into. Those
//! three each wrote *rendered output* — an SVG path per glyph per size, a PNG
//! of the whole repertoire — which is why `sample.html` reached 6.6 MB for a
//! font whose WOFF2 is 250 KB. This page embeds the **font** instead and lets
//! the browser draw: two `@font-face` rules (the bitmap build and the vector
//! build of the primary face) plus one JSON blob saying which characters exist,
//! which of them the font maps, and what block each falls in. Everything that
//! used to be markup is now built by `demo.js` from that blob.
//!
//! # What the specimen shows
//!
//! The grid follows the editor's specimen panel (`crate::specimen`) — one
//! section per block, a heading with the block's range and coverage, and cells
//! in code point order — with four deliberate differences, all of them because
//! this page has no font directory to jump into and no options menu to open:
//!
//! 1. Sixteen columns, always, and rows aligned to `cp & !0xF`. A page that
//!    cannot be resized has no reason to reflow, and the alignment is what
//!    turns the grid into a code chart: the column a character sits in is the
//!    low nibble of its code point, which is how a reader finds one.
//! 2. Undeclared characters are always shown — the editor's
//!    `show_undeclared` with no way to turn it off, since the gaps in the
//!    coverage are most of what a reader of this page came to see. Which code
//!    points count as characters is the editor's rule exactly
//!    ([`CharProps::is_assigned`](crate::ucd::CharProps::is_assigned)): the
//!    UCD outside Private Use, and the source's own `prop` lines inside it.
//! 3. Variation sequences and remap-only glyphs get no cell yet. They need an
//!    interface of their own here (the editor puts them inline, which reads
//!    only because a click can go to the source).
//! 4. Nothing is clickable. There is nowhere to click *to*.
//!
//! The exclusion rule is kept as it is: a row whose every character is
//! `exclude-from-sample` collapses, and a run of them becomes one marker,
//! so an excluded range still reads differently from one the source never
//! mentions.

use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::document::Document;
use crate::render::sample::{SampleSource, base64_encode};
use crate::ucd::{BlockMap, CharProps, format_block_range};

/// The two flavors of one face, WOFF2-encoded: what the page embeds and
/// switches between. See [`crate::render::build_face_ttf_pair`].
pub struct DemoFonts<'a> {
    pub bitmap_woff2: &'a [u8],
    pub vector_woff2: &'a [u8],
    /// The bitmap build's TTF. Not embedded — the WOFF2 above is — but read for
    /// its `hmtx`, which is where the zero-advance characters come from. Asking
    /// the built font is the same rule the editor's placeholders follow: what
    /// gets a circle is what the font gives no advance, not what Unicode calls
    /// a mark.
    pub bitmap_ttf: &'a [u8],
}

/// A cell the source maps to a glyph, as opposed to one only listed because the
/// character exists.
const CELL_DECLARED: u32 = 1;
/// A cell whose character is `exclude-from-sample`.
const CELL_EXCLUDED: u32 = 2;
/// A cell whose character the font gives no advance, so the page draws a dotted
/// circle for it to sit on. See `demo.css`'s `.dc`, and
/// [`crate::editor::annotations`] for why the circle is drawn and not typed.
const CELL_ZERO_ADVANCE: u32 = 4;

#[derive(serde::Serialize)]
struct DemoMeta {
    family: String,
    subfamily: String,
    version: String,
    face: String,
    height: u16,
    ascent: u16,
    descent: u16,
    features: Vec<String>,
    /// How many characters the face maps — the count the header states.
    mapped: usize,
}

/// One block's worth of cells.
///
/// The cells are runs rather than a list: a filled block is contiguous almost
/// everywhere, so `[start, len, flags]` triples are a few dozen numbers where
/// the code points themselves would be tens of thousands. The page lays them
/// out by code point, so a hole in the runs *is* an empty slot in the chart and
/// needs nothing said about it.
#[derive(serde::Serialize)]
struct DemoBlock {
    name: String,
    /// `null` for the one section holding the code points no block covers —
    /// there is no range for them to be a fraction of.
    range: Option<String>,
    start: u32,
    end: u32,
    /// `[declared, total]` — the coverage the heading states, on the same rule
    /// as the editor's: only when every mapped code point of the block is a
    /// character, so the fraction can never read over 100%.
    coverage: Option<[usize; 2]>,
    runs: Vec<[u32; 3]>,
}

#[derive(serde::Serialize)]
struct DemoData {
    meta: DemoMeta,
    blocks: Vec<DemoBlock>,
    /// Character names, for the mapped characters alone: hex code point to
    /// name. The whole repertoire's names would be several times the size of
    /// everything else here, and an undeclared cell has its code point to say
    /// what it is.
    ///
    /// The *derivable* names are not in here at all — see [`DemoData::name_runs`]
    /// and the Hangul rule in `demo.js`. They were 720 KB of the first page
    /// this wrote, against 770 KB for both fonts together: a font whose
    /// repertoire is mostly ideographs and syllables is mostly names that a
    /// dozen lines of JavaScript can spell for itself.
    names: BTreeMap<String, String>,
    /// `[start, len, prefix]` — the code point ranges whose character name is
    /// its prefix followed by the code point in hexadecimal, which is how the
    /// UCD names every ideograph. The page spells them out rather than reading
    /// them.
    name_runs: Vec<(u32, u32, String)>,
}

fn collect(src: &SampleSource, docs: &[&Document], bitmap_ttf: &[u8]) -> DemoData {
    let meta = crate::meta::FontMeta::collect(docs);
    let faces = crate::faces::FaceSet::collect(docs);
    let face = faces.primary();
    let face_meta = crate::meta::FontMeta::for_face(docs, Some(face.id.as_str()));
    let blocks_map = BlockMap::collect(docs);
    let props: &CharProps = src.char_props();
    let declared = src.cmap();
    let excluded = src.excluded();
    let zero_advance = zero_advance_codepoints(bitmap_ttf, declared);

    // Group the mapped characters by block, exactly as the specimen does: a
    // code point no block covers goes into one section at the end rather than
    // having a range invented for it, and that section is never filled.
    let mut by_block: BTreeMap<(u32, u32), (String, Vec<u32>)> = BTreeMap::new();
    let mut no_block: Vec<u32> = Vec::new();
    for &cp in declared.keys() {
        match blocks_map.block_of(cp) {
            Some(b) => by_block
                .entry((b.start, b.end))
                .or_insert_with(|| (b.name.to_string(), Vec::new()))
                .1
                .push(cp),
            None => no_block.push(cp),
        }
    }

    // A `prop block` claim nested inside a UCD block takes its code points out
    // of the outer one; both bounds are compared for the same reason the
    // editor compares both — a claim can share its start with the block it
    // overrides.
    let in_block = |cp: u32, start: u32, end: u32| {
        blocks_map
            .block_of(cp)
            .is_some_and(|b| (b.start, b.end) == (start, end))
    };

    let mut out_blocks: Vec<DemoBlock> = Vec::new();
    for ((start, end), (name, cps)) in by_block {
        let coverage = cps.iter().all(|&cp| props.is_assigned(cp)).then(|| {
            let total = (start..=end)
                .filter(|&cp| in_block(cp, start, end) && props.is_assigned(cp))
                .count();
            [cps.len(), total]
        });
        let members = (start..=end).filter(|&cp| {
            in_block(cp, start, end) && (declared.contains_key(&cp) || props.is_assigned(cp))
        });
        out_blocks.push(DemoBlock {
            name,
            range: Some(format_block_range(start, end)),
            start,
            end,
            coverage,
            runs: runs_of(members, declared, excluded, &zero_advance),
        });
    }
    if !no_block.is_empty() {
        let (start, end) = (no_block[0], *no_block.last().unwrap());
        out_blocks.push(DemoBlock {
            name: "No Block".to_string(),
            range: None,
            start,
            end,
            coverage: None,
            runs: runs_of(no_block.iter().copied(), declared, excluded, &zero_advance),
        });
    }

    let (names, name_runs) = collect_names(declared.keys().copied(), props);

    DemoData {
        meta: DemoMeta {
            family: face_meta.family().to_string(),
            subfamily: face_meta.subfamily().to_string(),
            version: face_meta.version_text(),
            face: face.id.clone(),
            height: meta.height(),
            ascent: meta.ascent(),
            descent: meta.descent(),
            features: src.features().to_vec(),
            mapped: declared.len(),
        },
        blocks: out_blocks,
        names,
        name_runs,
    }
}

/// Whether `cp` is a Hangul syllable, whose name the page composes from the
/// jamo it decomposes into rather than being told.
fn is_hangul_syllable(cp: u32) -> bool {
    (0xAC00..=0xD7A3).contains(&cp)
}

/// The character names to embed: the ones that have to be written out, and the
/// prefix runs that stand for the ones that do not.
///
/// A name ending in the code point's own hexadecimal — `CJK UNIFIED
/// IDEOGRAPH-4E00`, and every other ideograph and unnamed-but-labelled
/// character the UCD spells that way — is a prefix and a number, so a range of
/// them is one entry here. Hangul syllables are dropped outright: their names
/// are the jamo they decompose into, which the page composes for itself and for
/// undeclared cells too.
#[allow(clippy::type_complexity)]
fn collect_names(
    cps: impl Iterator<Item = u32>,
    props: &CharProps,
) -> (BTreeMap<String, String>, Vec<(u32, u32, String)>) {
    let mut names = BTreeMap::new();
    let mut runs: Vec<(u32, u32, String)> = Vec::new();
    for cp in cps {
        if is_hangul_syllable(cp) {
            continue;
        }
        let Some(name) = props.name(cp) else {
            continue;
        };
        // The `-` is required, so a name that merely happens to end in the
        // digits of its own code point is written out like any other.
        match name
            .strip_suffix(&format!("{cp:X}"))
            .filter(|p| p.ends_with('-'))
        {
            Some(prefix) => match runs.last_mut() {
                Some(run) if run.0 + run.1 == cp && run.2 == prefix => run.1 += 1,
                _ => runs.push((cp, 1, prefix.to_string())),
            },
            None => {
                names.insert(format!("{cp:X}"), name);
            }
        }
    }
    (names, runs)
}

/// Ascending code points to `[start, len, flags]` runs, breaking wherever the
/// code points are not consecutive or the flags differ.
fn runs_of(
    cps: impl Iterator<Item = u32>,
    declared: &BTreeMap<u32, String>,
    excluded: &std::collections::BTreeSet<u32>,
    zero_advance: &std::collections::BTreeSet<u32>,
) -> Vec<[u32; 3]> {
    let mut runs: Vec<[u32; 3]> = Vec::new();
    for cp in cps {
        let mut flags = 0;
        if declared.contains_key(&cp) {
            flags |= CELL_DECLARED;
        }
        if excluded.contains(&cp) {
            flags |= CELL_EXCLUDED;
        }
        if zero_advance.contains(&cp) {
            flags |= CELL_ZERO_ADVANCE;
        }
        match runs.last_mut() {
            Some(run) if run[0] + run[1] == cp && run[2] == flags => run[1] += 1,
            _ => runs.push([cp, 1, flags]),
        }
    }
    runs
}

/// The mapped code points the font gives no horizontal advance.
///
/// Read from the built font rather than derived from the source: it is what the
/// browser will lay the page out with, so it is what decides whether a cell
/// needs a circle to hold it open.
fn zero_advance_codepoints(
    ttf: &[u8],
    declared: &BTreeMap<u32, String>,
) -> std::collections::BTreeSet<u32> {
    let Ok(face) = rustybuzz::ttf_parser::Face::parse(ttf, 0) else {
        return Default::default();
    };
    declared
        .keys()
        .copied()
        .filter(|&cp| {
            char::from_u32(cp)
                .and_then(|c| face.glyph_index(c))
                .and_then(|gid| face.glyph_hor_advance(gid))
                == Some(0)
        })
        .collect()
}

pub fn write_demo_html(
    w: &mut dyn Write,
    src: &SampleSource,
    docs: &[&Document],
    fonts: DemoFonts<'_>,
) -> io::Result<()> {
    let data = collect(src, docs, fonts.bitmap_ttf);
    let title = format!("{} \u{2014} specimen", data.meta.family);
    // `</` inside the blob would end the script element early whatever it sits
    // in; JSON has no other way to spell a slash, so it is escaped here rather
    // than being trusted not to occur.
    let json = serde_json::to_string(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        .replace("</", "<\\/");
    let feature_css = if data.meta.features.is_empty() {
        "normal".to_string()
    } else {
        data.meta
            .features
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(",")
    };

    write!(
        w,
        "<!doctype html>\n<html lang=en><head><meta charset=utf-8>\n\
         <meta name=viewport content=\"width=device-width,initial-scale=1\">\n\
         <title>{title}</title>\n<style>\n\
         @font-face{{font-family:'UnisonBitmap';src:url(data:font/woff2;base64,{bitmap}) format('woff2');font-feature-settings:{feature_css}}}\n\
         @font-face{{font-family:'UnisonVector';src:url(data:font/woff2;base64,{vector}) format('woff2');font-feature-settings:{feature_css}}}\n\
         {css}</style>\n</head><body>\n\
         <script type=\"application/json\" id=\"demo-data\">{json}</script>\n\
         <script>\n{js}</script>\n</body></html>\n",
        title = html_escape(&title),
        bitmap = base64_encode(fonts.bitmap_woff2),
        vector = base64_encode(fonts.vector_woff2),
        css = include_str!("demo.css"),
        js = include_str!("demo.js"),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cps(list: &[u32]) -> BTreeMap<u32, String> {
        list.iter().map(|&cp| (cp, format!("g{cp:x}"))).collect()
    }

    #[test]
    fn runs_merge_consecutive_code_points_with_equal_flags() {
        let declared = cps(&[1, 2, 3, 10]);
        let excluded: std::collections::BTreeSet<u32> = [3].into_iter().collect();
        let zero: std::collections::BTreeSet<u32> = [2].into_iter().collect();
        let runs = runs_of([0, 1, 2, 3, 4, 10].into_iter(), &declared, &excluded, &zero);
        assert_eq!(
            runs,
            vec![
                [0, 1, 0],
                [1, 1, CELL_DECLARED],
                // A zero-advance cell breaks the run the way any other flag
                // does, so the marks of a block cost one run between them.
                [2, 1, CELL_DECLARED | CELL_ZERO_ADVANCE],
                [3, 1, CELL_DECLARED | CELL_EXCLUDED],
                [4, 1, 0],
                // A gap in the code points breaks the run even though the
                // flags match: the page lays the cells out by code point.
                [10, 1, CELL_DECLARED],
            ]
        );
    }

    /// The circle follows the *font*, not the character: what gets one is what
    /// `hmtx` gives no advance.
    #[test]
    fn zero_advance_comes_from_the_built_fonts_own_metrics() {
        let input = "\
glyph pix 1 1
@@
glyph a 2 2
ref pix
glyph mark mark advance 0
ref pix
map A = a
map U+0301 = mark
";
        let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let ttf = crate::render::ttf_builder::build_font_from_documents(&[&doc])
            .expect("font should build");
        let declared = cps(&[u32::from(b'A'), 0x301]);
        let zero = zero_advance_codepoints(&ttf, &declared);
        assert!(!zero.contains(&u32::from(b'A')));
        assert!(zero.contains(&0x301), "an `advance 0` glyph is one");
    }

    #[test]
    fn names_that_end_in_their_own_code_point_become_prefix_runs() {
        let props = CharProps::default();
        // U+4E00..4E02 are `CJK UNIFIED IDEOGRAPH-4E00` and so on, so all
        // three are one run; U+0041 is a name of its own.
        let (names, runs) = collect_names([0x41, 0x4e00, 0x4e01, 0x4e02].into_iter(), &props);
        assert_eq!(
            names.get("41").map(String::as_str),
            Some("LATIN CAPITAL LETTER A")
        );
        assert_eq!(names.len(), 1);
        assert_eq!(
            runs,
            vec![(0x4e00, 3, "CJK UNIFIED IDEOGRAPH-".to_string())]
        );
    }

    #[test]
    fn hangul_syllable_names_are_left_to_the_page() {
        let props = CharProps::default();
        let (names, runs) = collect_names([0xac00, 0xd7a3].into_iter(), &props);
        assert!(names.is_empty(), "{names:?}");
        assert!(runs.is_empty(), "{runs:?}");
    }
}
