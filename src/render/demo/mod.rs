//! `demo.html`: the font's one self-contained demonstration page.
//!
//! It is what the three sample pages it replaced grew into. Those each wrote
//! *rendered output* — an SVG path per glyph per size, a PNG of the whole
//! repertoire — which is why the glyph-chart page reached 6.6 MB for a font
//! whose WOFF2 is 250 KB. This page embeds the **font** instead and lets
//! the browser draw: one `@font-face` rule — the primary face as a variable
//! font carrying both drawings, switched by the `BMAP` axis — plus one JSON
//! blob saying which characters exist, which of them the font maps, and what
//! block each falls in. Everything that used to be markup is now built by
//! `demo.js` from that blob.
//!
//! The font used to be two files and two `@font-face` rules, one per flavor.
//! One variable font is a little *smaller* than the pair (the two static faces
//! each carried a whole `glyf`; this carries one plus `gvar`), and it is one
//! fewer thing for the page to keep in step: the two drawings share glyph ids
//! and metrics because they are one glyph set.
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
//! 4. A block longer than 0x100 code points is *folded* in the middle. A code
//!    chart of a whole font is read by scrolling, and the one thing it cannot
//!    afford is ten thousand identical rows between two blocks; the fold keeps
//!    a block's two ends and puts everything between them one click away — see
//!    `demo.js`. The source has no say in which blocks fold, and the editor's
//!    specimen folds by the same rule (`crate::specimen`).
//!
//! # The sample panel
//!
//! A code chart says which characters the font has; it cannot say what they
//! look like as running text, which is the other half of what a specimen is
//! read for. The panel pinned to the bottom of the page is that half: a list of
//! the texts the page carries on the left, and the selected one — editable — on
//! the right. It collapses to its own title bar, since the chart is what the
//! page is mostly for.
//!
//! Three things it deliberately does not do. It has no size or mode control of
//! its own: the header's two drive it, so the running text and the chart are
//! always the same drawing at the same size and can be compared. It does not
//! invent sample text: every group on the list is a [`sample`](crate::samples)
//! line of the source, the generated ones included — a
//! [`udhr-article1`](crate::samples::SampleMode::UdhrArticle1) line is filled
//! in from [`crate::render::sample::udhr_selection`] here, written out per
//! translation rather than as one blob, because the *list* is what makes the
//! panel worth having. And it does not keep what the reader types anywhere but
//! `sessionStorage`, per sample: an edit survives a reload and a jump to
//! another block, and nothing on this page outlives the tab.
//!
//! # What the blob costs
//!
//! Everything in it is *modelled* before it is written — nothing here is a
//! compressed stream, only a shape chosen so that what repeats is written
//! once. Three of those pay for themselves: a block's cells are runs written
//! as distances rather than code points ([`DemoBlock::runs`]), the character
//! names are front-coded against each other ([`DemoNames`]), and a range the
//! UCD names by rule is one entry however few of it the font draws
//! ([`widen_runs`]). Together they took the blob of this font from 587 KB to
//! 197 KB, and the page from 1.5 MB to 1.1 MB.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use crate::document::Document;
use crate::render::sample::{SampleSource, base64_encode};
use crate::ucd::{BlockMap, CharProps, format_block_range};

/// One face carrying both drawings, WOFF2-encoded: what the page embeds and
/// switches between. See [`crate::render::build_face_variable`].
///
/// It used to be two files and two `@font-face` rules, one per flavor. One
/// variable font is the same two drawings with the *same* glyph ids and the
/// same metrics, which is one fewer thing for the page to keep in step, and it
/// costs about 16 KB against the pair it replaces.
pub struct DemoFonts<'a> {
    pub woff2: &'a [u8],
    /// The same font as TTF. Not embedded — the WOFF2 above is — but read for
    /// its `hmtx`, which is where the zero-advance characters come from. Asking
    /// the built font is the same rule the editor's placeholders follow: what
    /// gets a circle is what the font gives no advance, not what Unicode calls
    /// a mark. Advances do not vary along the axis, so one reading serves both
    /// drawings.
    pub ttf: &'a [u8],
}

/// A cell the source maps to a glyph, as opposed to one only listed because the
/// character exists.
const CELL_DECLARED: u32 = 1;
/// A cell whose character the font gives no advance, so the page draws a dotted
/// circle for it to sit on. See `demo.css`'s `.dc`, and
/// [`crate::editor::annotations`] for why the circle is drawn and not typed.
const CELL_ZERO_ADVANCE: u32 = 2;

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
/// everywhere, so a few dozen runs stand for what would be tens of thousands of
/// code points. The page lays them out by code point, so a hole in the runs
/// *is* an empty slot in the chart and needs nothing said about it.
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
    /// `gap,len,flags` triples in lowercase hexadecimal, separated by `;`; see
    /// [`runs_of`]. A run's *gap* is how far its first code point sits past the
    /// end of the run before it (past `start`, for the first one), which is what
    /// keeps the numbers one or two digits long: written out in full, the
    /// eleven thousand runs of this font were 188 KB of the page.
    runs: String,
}

#[derive(serde::Serialize)]
struct DemoData {
    meta: DemoMeta,
    blocks: Vec<DemoBlock>,
    /// The character names that have to be written out; see [`DemoNames`].
    names: DemoNames,
    /// `[start, len, prefix]` — the code point ranges whose character name is
    /// its prefix followed by the code point in hexadecimal, which is how the
    /// UCD names every ideograph. The page spells them out rather than reading
    /// them.
    name_runs: Vec<(u32, u32, String)>,
    /// The ready-made texts the sample panel offers; empty when the build was
    /// given no `-d` directory to read them from.
    samples: Vec<DemoSampleGroup>,
}

/// One heading's worth of ready-made sample texts.
///
/// A group is a *body of data the page carries*, as against the empty sample a
/// reader types into: it has a title of its own because a hundred and nineteen
/// translations of one paragraph are one thing on the list and not a hundred
/// and nineteen. Every group is one `sample` label of the source, whether it
/// wrote the texts or named a mode that writes them.
#[derive(serde::Serialize)]
struct DemoSampleGroup {
    title: String,
    /// What the group is, spelled out where the title cannot be. Empty unless
    /// the group came from a generated mode: a label the source wrote is the
    /// whole of what it says, where `UDHR Article 1` over a list of language
    /// names is not.
    #[serde(skip_serializing_if = "String::is_empty")]
    note: String,
    /// The items' ids are UDHR translation keys the page turns into language
    /// names, which is what a
    /// [`udhr-article1`](crate::samples::SampleMode::UdhrArticle1) group is.
    /// Every other group's ids are the sublabels as written, and the page
    /// prints them.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    lang: bool,
    /// The text the heading itself carries, from a `sample LABEL` line with no
    /// sublabel; empty when the heading is only a heading. It is what makes the
    /// title on the list a thing to click.
    #[serde(skip_serializing_if = "String::is_empty")]
    text: String,
    /// Whether that text is a [`matrix`](crate::samples::SampleMode::Matrix),
    /// which the page expands for itself.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    matrix: bool,
    items: Vec<DemoSample>,
}

#[derive(serde::Serialize)]
struct DemoSample {
    /// What the sample is keyed and — for a source-written group — named by:
    /// the UDHR's own key for a translation, or the `sample` line's sublabel.
    id: String,
    /// The UDHR's own name for the translation, for the numeric keys `Intl`
    /// cannot name. Carried per item and not as a table because it is one
    /// string per translation the page already carries a paragraph for. Empty
    /// where the id is already the name.
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    /// The text as the source wrote it — a `matrix` is *not* expanded here.
    /// The page expands it (`demo.js`'s `sampleText`), because the product is
    /// what the mode exists to avoid writing out: the blob carries the four
    /// lines an author typed, not the four thousand cells they stand for.
    text: String,
    /// Whether `text` is a [`matrix`](crate::samples::SampleMode::Matrix).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    matrix: bool,
}

/// The sample texts to embed: the source's own [`sample`](crate::samples)
/// lines, in the order it writes them.
///
/// A line whose mode is a generated one carries no text of its own, and is
/// filled in here — this is the half of the build that has the `-d` directory
/// the generators read. A hundred-odd translations of one paragraph are a
/// *group* and not a text, which is why `udhr-article1` replaces a heading's
/// items where `subdivision-flags` replaces one text.
///
/// The data directory is optional for every other part of this page, so a build
/// with no `-d` still writes a demo. A generated group it cannot fill is
/// dropped rather than offered empty: a name on the list that opens onto
/// nothing is worse than one absence.
fn collect_samples(src: &SampleSource, data_dir: Option<&Path>) -> Vec<DemoSampleGroup> {
    use crate::samples::SampleMode;

    // Read once for the whole page, however many lines ask: both are a file
    // read and a parse, and a source may well offer one text under two labels.
    let udhr = || {
        data_dir
            .and_then(|dir| crate::render::sample::udhr_selection(dir, src.cmap()).ok())
            .unwrap_or_default()
    };
    let flags = || {
        data_dir
            .and_then(crate::render::sample::subdivisions_path)
            .and_then(|path| crate::render::sample::subdivision_flags_text(&path).ok())
            .unwrap_or_default()
    };
    let udhr = std::sync::LazyLock::new(udhr);
    let flags = std::sync::LazyLock::new(flags);

    let mut groups: Vec<DemoSampleGroup> = Vec::new();
    for group in src.samples().groups.iter().cloned() {
        let heading = group.text.as_ref().map(|t| t.mode).unwrap_or_default();
        if heading == SampleMode::UdhrArticle1 {
            if udhr.is_empty() {
                continue;
            }
            groups.push(DemoSampleGroup {
                title: group.label,
                note: UDHR_NOTE.to_string(),
                lang: true,
                text: String::new(),
                matrix: false,
                items: udhr
                    .iter()
                    .map(|e| DemoSample {
                        id: e.lang.clone(),
                        name: e.name.clone(),
                        text: e.text.clone(),
                        matrix: false,
                    })
                    .collect(),
            });
            continue;
        }
        let text = |mode: SampleMode, raw: String| match mode {
            SampleMode::SubdivisionFlags => flags.clone(),
            _ => raw,
        };
        let items: Vec<DemoSample> = group
            .items
            .into_iter()
            .map(|item| DemoSample {
                // The sublabel is both the key an edit is stored under and the
                // name on the list: it is unique within its group (`issues`
                // holds the source to that), so it needs no second string.
                id: item.sublabel,
                name: String::new(),
                matrix: item.text.mode == SampleMode::Matrix,
                text: text(item.text.mode, item.text.raw),
            })
            // A generated text the build could not assemble is no text.
            .filter(|item| !item.text.is_empty())
            .collect();
        let heading_text = group.text.map(|t| text(t.mode, t.raw)).unwrap_or_default();
        if items.is_empty() && heading_text.is_empty() {
            continue;
        }
        groups.push(DemoSampleGroup {
            title: group.label,
            note: String::new(),
            lang: false,
            matrix: heading == SampleMode::Matrix,
            text: heading_text,
            items,
        });
    }
    groups
}

/// What the `udhr-article1` group is, spelled out: its title is a label the
/// source chose, and a list of language names says nothing about where the
/// paragraph comes from or why these translations and not others.
const UDHR_NOTE: &str = "Article 1 of the Universal Declaration of Human Rights, in every \
                         translation this font can draw whole";

/// The character names the page cannot spell for itself, front-coded.
///
/// Only the *mapped* characters are here — the whole repertoire's names would
/// be several times the size of everything else on the page, and an undeclared
/// cell has its code point to say what it is — and only the ones nothing else
/// accounts for: the ideographs are [`DemoData::name_runs`] and the Hangul
/// syllables are their jamo, both spelled by `demo.js`. Those two were 720 KB
/// of the first page this wrote, against 770 KB for both fonts together.
///
/// What is left is still mostly repetition, since character names are written
/// to sort together — a hundred lines of `LATIN SMALL LETTER …` in a row — so
/// each name is stored as what it does *not* share with the one before it.
/// That is a model, not an encoding: it halves the blob before the transport's
/// own compression sees it, and costs the page one `slice` per name.
#[derive(serde::Serialize)]
struct DemoNames {
    /// The code points, ascending, each as its distance from the one before it
    /// (from 0, for the first) in lowercase hexadecimal, separated by `,`.
    cps: String,
    /// One entry per code point, in that order: a single base-62 digit saying
    /// how many leading characters the name shares with the previous one,
    /// followed by the rest of it. The shared count is measured in ASCII
    /// characters alone — a `prop` line may name a character anything at all,
    /// and only ASCII is one JavaScript string index per character — and capped
    /// at what one digit can say.
    text: Vec<String>,
}

/// The digits [`DemoNames::text`] writes a shared-prefix length with, and so
/// the longest one it can state. `demo.js` decodes with `indexOf` over the same
/// string.
const B62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

impl DemoNames {
    fn front_code(names: &[(u32, String)]) -> DemoNames {
        let mut cps = String::new();
        let mut text = Vec::with_capacity(names.len());
        let mut prev_cp = 0u32;
        let mut prev = "";
        for (cp, name) in names {
            if !cps.is_empty() {
                cps.push(',');
            }
            cps.push_str(&format!("{:x}", cp - prev_cp));
            prev_cp = *cp;
            let shared = name
                .bytes()
                .zip(prev.bytes())
                .take(B62.len() - 1)
                .take_while(|(a, b)| a == b && a.is_ascii())
                .count();
            text.push(format!("{}{}", B62[shared] as char, &name[shared..]));
            prev = name;
        }
        DemoNames { cps, text }
    }
}

fn collect(
    src: &SampleSource,
    docs: &[&Document],
    bitmap_ttf: &[u8],
    data_dir: Option<&Path>,
) -> DemoData {
    let meta = crate::meta::FontMeta::collect(docs);
    let faces = crate::faces::FaceSet::collect(docs);
    let face = faces.primary();
    let face_meta = crate::meta::FontMeta::for_face(docs, Some(face.id.as_str()));
    let blocks_map = BlockMap::collect(docs);
    let props: &CharProps = src.char_props();
    let declared = src.cmap();
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
            runs: runs_of(members, start, declared, &zero_advance),
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
            runs: runs_of(no_block.iter().copied(), start, declared, &zero_advance),
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
        samples: collect_samples(src, data_dir),
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
///
/// A run is not cut where the *font* stops: the mapped ideographs of a real
/// font are thousands of short stretches with gaps between them, which wrote
/// out as thousands of copies of the same prefix (202 KB of one page here).
/// A run therefore grows over a gap whenever nothing in the gap is named
/// anything else — the same reasoning the Hangul rule rests on, that a whole
/// UCD range is one naming rule. What a gap may hold is a code point the UCD
/// names the same way (mapped or not) or one it assigns no character at all,
/// and an unassigned code point never gets a cell, so nothing on the page can
/// read a name it was not meant to have. The undeclared ideographs inside the
/// span gain a name they had no entry for before.
fn collect_names(
    cps: impl Iterator<Item = u32>,
    props: &CharProps,
) -> (DemoNames, Vec<(u32, u32, String)>) {
    let mut names: Vec<(u32, String)> = Vec::new();
    let mut runs: Vec<(u32, u32, String)> = Vec::new();
    // Ascending, so a run only ever grows at its end.
    for cp in cps {
        if is_hangul_syllable(cp) {
            continue;
        }
        let Some(name) = props.name(cp) else {
            continue;
        };
        match algorithmic_prefix(&name, cp) {
            Some(prefix) => match runs.last_mut() {
                Some(run)
                    if run.2 == prefix
                        && (run.0 + run.1 == cp
                            || gap_is_free(props, run.0 + run.1, cp, prefix)) =>
                {
                    run.1 = cp + 1 - run.0;
                }
                _ => runs.push((cp, 1, prefix.to_string())),
            },
            None => names.push((cp, name)),
        }
    }
    widen_runs(&mut runs, props);
    (DemoNames::front_code(&names), runs)
}

/// Grow each run out to the whole range the UCD names that way, past the code
/// points the font happens to map.
///
/// A run is born spanning the *mapped* ideographs, which is an accident of the
/// font; the naming rule is the range's. Widening it costs a few bytes and
/// gives the undeclared cells of the range — most of a code chart of
/// ideographs — the name they describe. Only code points the same rule names
/// are swallowed here, unlike a gap *inside* a run, since there is no mapped
/// code point past the edge to say where the rule ought to stop.
fn widen_runs(runs: &mut [(u32, u32, String)], props: &CharProps) {
    let named = |cp: u32, prefix: &str| {
        props
            .name(cp)
            .is_some_and(|name| algorithmic_prefix(&name, cp) == Some(prefix))
    };
    // A run never grows into the one beside it: the two are different rules,
    // and the page reads the runs in order.
    let mut floor = 0;
    for i in 0..runs.len() {
        let ceiling = runs.get(i + 1).map_or(char::MAX as u32, |next| next.0 - 1);
        let (start, len, prefix) = &mut runs[i];
        while *start > floor && named(*start - 1, prefix) {
            *start -= 1;
            *len += 1;
        }
        while *start + *len <= ceiling && named(*start + *len, prefix) {
            *len += 1;
        }
        floor = *start + *len;
    }
}

/// The prefix of `name` when it is `PREFIX-` followed by `cp` in hexadecimal,
/// which is how the UCD names a whole range at once.
///
/// The `-` is required, so a name that merely happens to end in the digits of
/// its own code point is written out like any other.
fn algorithmic_prefix(name: &str, cp: u32) -> Option<&str> {
    name.strip_suffix(&format!("{cp:X}"))
        .filter(|p| p.ends_with('-'))
}

/// Whether `from..to` may be swallowed by a run of `prefix`: every code point
/// in it is either named the same way or is no character at all.
fn gap_is_free(props: &CharProps, from: u32, to: u32, prefix: &str) -> bool {
    (from..to).all(|cp| match props.name(cp) {
        Some(name) => algorithmic_prefix(&name, cp) == Some(prefix),
        None => !props.is_assigned(cp),
    })
}

/// Ascending code points to `gap,len,flags` runs, breaking wherever the code
/// points are not consecutive or the flags differ. See [`DemoBlock::runs`] for
/// the written form; `origin` is what the first run's gap is measured from.
fn runs_of(
    cps: impl Iterator<Item = u32>,
    origin: u32,
    declared: &BTreeMap<u32, String>,
    zero_advance: &std::collections::BTreeSet<u32>,
) -> String {
    let mut out = String::new();
    // The end of the last run written, which the next run's gap is measured
    // from, and the run being extended.
    let mut prev_end = origin;
    let mut cur: Option<(u32, u32, u32)> = None;
    let flush = |out: &mut String, prev_end: &mut u32, (start, len, flags): (u32, u32, u32)| {
        if !out.is_empty() {
            out.push(';');
        }
        out.push_str(&format!("{:x},{len:x},{flags:x}", start - *prev_end));
        *prev_end = start + len;
    };
    for cp in cps {
        let mut flags = 0;
        if declared.contains_key(&cp) {
            flags |= CELL_DECLARED;
        }
        if zero_advance.contains(&cp) {
            flags |= CELL_ZERO_ADVANCE;
        }
        match cur {
            Some((start, len, f)) if start + len == cp && f == flags => {
                cur = Some((start, len + 1, f));
            }
            Some(run) => {
                flush(&mut out, &mut prev_end, run);
                cur = Some((cp, 1, flags));
            }
            None => cur = Some((cp, 1, flags)),
        }
    }
    if let Some(run) = cur {
        flush(&mut out, &mut prev_end, run);
    }
    out
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
    data_dir: Option<&Path>,
) -> io::Result<()> {
    let data = collect(src, docs, fonts.ttf, data_dir);
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
         @font-face{{font-family:'Unison';src:url(data:font/woff2;base64,{font}) format('woff2');font-feature-settings:{feature_css}}}\n\
         {css}</style>\n</head><body>\n\
         <script type=\"application/json\" id=\"demo-data\">{json}</script>\n\
         <script>\n{js}</script>\n</body></html>\n",
        title = html_escape(&title),
        font = base64_encode(fonts.woff2),
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
        let zero: std::collections::BTreeSet<u32> = [2].into_iter().collect();
        let runs = runs_of([0, 1, 2, 3, 4, 10].into_iter(), 0, &declared, &zero);
        assert_eq!(
            runs,
            concat!(
                "0,1,0;", "0,1,1;",
                // A zero-advance cell breaks the run the way any other flag
                // does, so the marks of a block cost one run between them.
                "0,1,3;", "0,1,1;", "0,1,0;",
                // A gap in the code points breaks the run even though the
                // flags match: the page lays the cells out by code point, and
                // the gap is the distance the run is written with.
                "5,1,1",
            )
        );
    }

    /// The first gap is measured from the block's own start, so a block high in
    /// the code space costs no more than one low in it.
    #[test]
    fn the_first_run_is_written_relative_to_the_blocks_start() {
        let declared = cps(&[0x20001]);
        let empty = Default::default();
        let runs = runs_of([0x20001].into_iter(), 0x20000, &declared, &empty);
        assert_eq!(runs, "1,1,1");
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
        // three are one run — widened to the range that names them, U+4E00..
        // 9FFF; U+0041 is a name of its own.
        let (names, runs) = collect_names([0x41, 0x4e00, 0x4e01, 0x4e02].into_iter(), &props);
        assert_eq!(names.cps, "41");
        assert_eq!(names.text, vec!["0LATIN CAPITAL LETTER A"]);
        assert_eq!(
            runs,
            vec![(
                0x4e00,
                0x9fff - 0x4e00 + 1,
                "CJK UNIFIED IDEOGRAPH-".to_string()
            )]
        );
    }

    /// The gaps a font leaves in a UCD range must not cut the run: the whole
    /// range is one naming rule, and what falls in a gap is either named by
    /// that rule or is not a character at all.
    #[test]
    fn a_prefix_run_grows_over_the_code_points_the_font_skips() {
        let props = CharProps::default();
        let (_, runs) = collect_names([0x4e00, 0x9fff].into_iter(), &props);
        assert_eq!(
            runs,
            vec![(
                0x4e00,
                0x9fff - 0x4e00 + 1,
                "CJK UNIFIED IDEOGRAPH-".to_string()
            )]
        );
    }

    /// A run stands for the whole range the UCD names that way, not for the
    /// stretch of it the font happens to draw: an undeclared ideograph is
    /// still an ideograph, and its cell says so.
    #[test]
    fn a_prefix_run_covers_the_range_and_not_just_the_mapped_part() {
        let props = CharProps::default();
        let (_, runs) = collect_names([0x4e05].into_iter(), &props);
        assert_eq!(
            runs,
            vec![(
                0x4e00,
                0x9fff - 0x4e00 + 1,
                "CJK UNIFIED IDEOGRAPH-".to_string()
            )]
        );
    }

    /// ... but a character named anything else in the gap does cut it, or the
    /// page would read it out under the run's prefix.
    #[test]
    fn a_named_character_in_the_gap_cuts_the_run() {
        let props = CharProps::default();
        // U+A000 YI SYLLABLE IT sits between the two ideograph ranges.
        let (_, runs) = collect_names([0x9fff, 0x20000].into_iter(), &props);
        assert_eq!(runs.len(), 2, "{runs:?}");
    }

    #[test]
    fn hangul_syllable_names_are_left_to_the_page() {
        let props = CharProps::default();
        let (names, runs) = collect_names([0xac00, 0xd7a3].into_iter(), &props);
        assert!(names.text.is_empty(), "{:?}", names.text);
        assert!(runs.is_empty(), "{runs:?}");
    }

    /// Each name is written as what it does not share with the one before it,
    /// and the shared count is a base-62 digit.
    #[test]
    fn names_are_front_coded_against_the_previous_one() {
        let names = DemoNames::front_code(&[
            (0x41, "LATIN CAPITAL LETTER A".to_string()),
            (0x42, "LATIN CAPITAL LETTER B".to_string()),
            (0x100, "LATIN CAPITAL LETTER A WITH MACRON".to_string()),
        ]);
        assert_eq!(names.cps, "41,1,be");
        assert_eq!(
            names.text,
            vec!["0LATIN CAPITAL LETTER A", "LB", "LA WITH MACRON"]
        );
    }

    /// A name a `prop` line writes may be anything; the shared count is one
    /// JavaScript string index per character, so it stops at the first
    /// non-ASCII byte rather than counting bytes the page would not agree on.
    #[test]
    fn a_shared_prefix_is_counted_in_ascii_alone() {
        let names = DemoNames::front_code(&[
            (1, "\u{ac00}FOO".to_string()),
            (2, "\u{ac00}FOO BAR".to_string()),
        ]);
        assert_eq!(names.text, vec!["0\u{ac00}FOO", "0\u{ac00}FOO BAR"]);
    }

    fn samples_of(src_text: &str, data_dir: Option<&Path>) -> Vec<DemoSampleGroup> {
        let doc = crate::document_io::parse_document_from_str(src_text, "test.unf".into()).unwrap();
        let resolution = crate::resolve::Resolution::compute(&[&doc]);
        let src = SampleSource::collect_with(&[&doc], &resolution).unwrap();
        collect_samples(&src, data_dir)
    }

    const MINIMAL: &str =
        "meta height 16\nmeta ascent 12\nmeta descent 4\n\nglyph a 1 1\n@\n\nmap A = a\n\n";

    /// A `udhr-article1` line is a heading with no text of its own; what the
    /// panel gets is one item per translation, keyed by the UDHR's own key.
    #[test]
    fn a_udhr_line_becomes_one_item_per_translation() {
        let dir = std::env::temp_dir().join(format!("uniform-demo-udhr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("udhr-article1.json"),
            r#"[{"lang":"eng","name":"English","text":"A"},
                {"lang":"zzz","name":"Nothing","text":"Z"}]"#,
        )
        .unwrap();

        let text = format!("{MINIMAL}sample `Article 1` : udhr-article1\n");
        let groups = samples_of(&text, Some(&dir));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Article 1", "the label is the source's");
        assert!(groups[0].lang, "the ids are language keys");
        assert!(
            !groups[0].note.is_empty(),
            "and the note says what they are"
        );
        assert_eq!(
            groups[0]
                .items
                .iter()
                .map(|i| (i.id.as_str(), i.text.as_str()))
                .collect::<Vec<_>>(),
            vec![("eng", "A")],
            "a translation the font cannot draw whole is not offered"
        );

        assert!(
            samples_of(&text, None).is_empty(),
            "a group the build cannot fill is dropped, not offered empty"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `subdivision-flags` line is one text, and is filled in where it is
    /// written — under a sublabel here, beside the source's own texts.
    #[test]
    fn a_subdivision_flags_line_becomes_one_text() {
        let dir = std::env::temp_dir().join(format!("uniform-demo-flags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("cldr-subdivisions-1.2.3.json"),
            r#"{"subdivisions":{"GB":["gbsct"]}}"#,
        )
        .unwrap();

        let text = format!(
            "{MINIMAL}sample F `Mine`\n|| written\nsample F `Subdivisions` : subdivision-flags\n"
        );
        let groups = samples_of(&text, Some(&dir));
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0]
                .items
                .iter()
                .map(|i| (i.id.as_str(), i.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Mine", "written"),
                (
                    "Subdivisions",
                    "GB \u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}"
                )
            ]
        );

        let groups = samples_of(&text, None);
        assert_eq!(
            groups[0].items.len(),
            1,
            "the text the source wrote survives a build with no data directory"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
