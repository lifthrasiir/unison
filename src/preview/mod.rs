//! Shaping the preview field's text with the built font.
//!
//! # One paragraph in, visual order out
//!
//! A backend is handed a whole [`Paragraph`] — one line of the preview, with
//! its bidi levels already resolved — and returns glyphs in the order they are
//! to be painted, left to right. It is *not* handed pre-split single-direction
//! runs, because two of the three backends do UAX #9 themselves and doing it
//! for them would replace the very part of the platform stack the preview
//! exists to proof; see [`bidi`] for the argument. What the shared path does
//! provide is [`Paragraph::char_levels`], so a backend that resolves its own
//! levels still labels its glyphs the way the caret code expects.
//!
//! [`shape_runs`] is the shared *implementation* a backend may opt into: it
//! splits the paragraph into runs that are one direction and one script each,
//! in visual order, and shapes them one at a time. Only rustybuzz uses it.

pub mod bidi;
pub mod cluster;
pub mod metrics;
pub mod rasterizer;
pub mod rustybuzz;
pub mod widget;

#[cfg(target_os = "macos")]
pub mod coretext;

#[cfg(target_os = "windows")]
pub mod directwrite;

use bidi::{BidiRun, ParagraphDirection};

/// Maps each UTF-16 code unit index of `text` to its char index.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_utf16_to_char_map(text: &str) -> Vec<usize> {
    let mut map = Vec::new();
    for (char_idx, ch) in text.chars().enumerate() {
        for _ in 0..ch.len_utf16() {
            map.push(char_idx);
        }
    }
    map
}

/// One line of the preview, with UAX #9 already run over it.
pub struct Paragraph<'a> {
    pub text: &'a str,
    /// What the caller asked the paragraph level to be. Only a backend that
    /// resolves its own levels has to be told; Core Text is the one that is.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code))]
    pub direction: ParagraphDirection,
    /// Level runs, in the visual order rule L2 puts them in.
    pub runs: Vec<BidiRun>,
    /// The resolved embedding level of each *character*, by char index. A
    /// backend that did its own reordering labels its glyphs from this rather
    /// than from `runs`, so the caret code sees one vocabulary whichever
    /// backend produced the glyphs. Read through [`Paragraph::level_of_char`].
    pub char_levels: Vec<u8>,
    /// The paragraph embedding level: what a caret with no character beside it
    /// falls back to.
    pub level: u8,
}

impl<'a> Paragraph<'a> {
    pub fn new(text: &'a str, direction: ParagraphDirection) -> Self {
        let line = bidi::split_bidi_runs(text, direction);
        let runs = line.runs;
        let mut char_levels = vec![line.paragraph_level; text.chars().count()];
        for run in &runs {
            let len = text[run.bytes.clone()].chars().count();
            for level in &mut char_levels[run.char_start..run.char_start + len] {
                *level = run.level;
            }
        }
        Self {
            text,
            direction,
            runs,
            char_levels,
            level: line.paragraph_level,
        }
    }

    /// The level to label a glyph with, given the character it came from.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code))]
    fn level_of_char(&self, char_idx: usize) -> u8 {
        self.char_levels
            .get(char_idx)
            .copied()
            .unwrap_or(self.level)
    }
}

#[derive(Clone, Debug)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    /// Char index within the paragraph of the character this glyph came from.
    /// In a right-to-left run these *decrease* along the visual order, which is
    /// how [`cluster`] tells one run from the next.
    pub cluster: usize,
    /// The resolved embedding level of the run this glyph belongs to; odd is
    /// right-to-left. See [`Paragraph::char_levels`].
    pub level: u8,
    pub x_advance: f32,
    #[allow(dead_code)]
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug)]
pub struct ShapeError(pub String);

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait ShaperBackend {
    fn name(&self) -> &'static str;

    /// Shape one paragraph, returning its glyphs **in visual order** — the
    /// order they are painted, left to right — with every glyph's `cluster`
    /// a char index into `para.text` and its `level` the run's.
    fn shape(
        &self,
        font_data: &[u8],
        para: &Paragraph<'_>,
        upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError>;
}

/// Shape `para` by handing `shape_run` one run at a time, each of which is a
/// single direction *and* a single script, visited in visual order.
///
/// A level run still has to be split by script before it reaches a shaper —
/// see [`crate::script_run`] for why — and inside a right-to-left level run
/// those script runs are visited back to front, since visual order is what the
/// concatenation has to come out in. The shaper is told the level rather than
/// just "backward" so that the glyphs can carry it out.
///
/// Cluster indices come back rebased onto the whole paragraph, and features
/// are clipped and rebased onto each run.
pub fn shape_runs(
    para: &Paragraph<'_>,
    features: &[Feature],
    mut shape_run: impl FnMut(&str, u8, &[Feature]) -> Result<Vec<ShapedGlyph>, ShapeError>,
) -> Result<Vec<ShapedGlyph>, ShapeError> {
    let mut glyphs = Vec::new();
    for level_run in &para.runs {
        let run_text = &para.text[level_run.bytes.clone()];
        let mut script_runs = crate::script_run::split_script_runs(run_text);
        if level_run.is_rtl() {
            script_runs.reverse();
        }
        for script_run in script_runs {
            let sub = &run_text[script_run.bytes.clone()];
            let char_start = level_run.char_start + script_run.char_start;
            let sub_features: Vec<Feature> = features
                .iter()
                .filter_map(|f| clip_feature(f, char_start, sub.chars().count()))
                .collect();
            let mut run_glyphs = shape_run(sub, level_run.level, &sub_features)?;
            to_visual_order(&mut run_glyphs, level_run.is_rtl());
            for mut glyph in run_glyphs {
                glyph.cluster += char_start;
                glyph.level = level_run.level;
                glyphs.push(glyph);
            }
        }
    }
    Ok(glyphs)
}

/// Put one run's glyphs into visual order, which for a backward run means
/// descending cluster with the glyphs of a single cluster left where they are —
/// a mark still follows its base.
///
/// This states the contract rather than trusting a shaper to have met it.
/// rustybuzz reverses a backward run itself, so this is a no-op there; the
/// DirectWrite analyzer's order is not something the documentation pins down,
/// and a run arriving in logical order made every glyph its own run in
/// [`cluster`], which is what this exists to stop.
fn to_visual_order(glyphs: &mut [ShapedGlyph], rtl: bool) {
    if !rtl || glyphs.len() < 2 {
        return;
    }
    // Cluster groups are contiguous in either order, so sorting the *glyphs*
    // by descending cluster — stably, so a group's own order survives — is the
    // same as reordering the groups.
    if glyphs.windows(2).all(|w| w[0].cluster >= w[1].cluster) {
        return;
    }
    glyphs.sort_by_key(|g| std::cmp::Reverse(g.cluster));
}

/// Restrict a feature's character range to a run and rebase it onto that run.
/// Returns `None` if the feature does not overlap the run at all.
fn clip_feature(feature: &Feature, char_start: usize, char_len: usize) -> Option<Feature> {
    let start = feature.start.max(char_start);
    let end = feature.end.min(char_start + char_len);
    (start < end).then(|| Feature {
        tag: feature.tag,
        value: feature.value,
        start: start - char_start,
        end: end - char_start,
    })
}

pub fn available_backends() -> Vec<Box<dyn ShaperBackend>> {
    let mut v: Vec<Box<dyn ShaperBackend>> = vec![Box::new(rustybuzz::RustyBuzzBackend)];
    #[cfg(target_os = "macos")]
    v.push(Box::new(coretext::CoreTextBackend));
    #[cfg(target_os = "windows")]
    v.push(Box::new(directwrite::DirectWriteBackend));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Stands in for a real shaper: emits one glyph per character, with the
    /// cluster index relative to the run it was handed, and records the runs
    /// it was called with. `logical_order` makes it behave like a shaper that
    /// leaves a backward run's glyphs in logical order.
    #[derive(Default)]
    struct Recorder {
        seen: RefCell<Vec<(String, u8)>>,
        logical_order: bool,
    }

    impl Recorder {
        fn shape(&self, text: &str, level: u8) -> Result<Vec<ShapedGlyph>, ShapeError> {
            self.seen.borrow_mut().push((text.to_string(), level));
            let chars: Vec<char> = text.chars().collect();
            // A backward run comes out of a shaper in visual order already.
            let order: Vec<usize> = if level % 2 == 1 && !self.logical_order {
                (0..chars.len()).rev().collect()
            } else {
                (0..chars.len()).collect()
            };
            Ok(order
                .into_iter()
                .map(|i| ShapedGlyph {
                    glyph_id: 0,
                    cluster: i,
                    level,
                    x_advance: 1.0,
                    y_advance: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                })
                .collect())
        }
    }

    /// `(runs the shaper saw, cluster index of each glyph in visual order)`.
    fn shape_recording(text: &str) -> (Vec<(String, u8)>, Vec<usize>) {
        shape_recording_dir(text, ParagraphDirection::Auto)
    }

    fn shape_recording_dir(
        text: &str,
        direction: ParagraphDirection,
    ) -> (Vec<(String, u8)>, Vec<usize>) {
        let para = Paragraph::new(text, direction);
        let recorder = Recorder::default();
        let glyphs = shape_runs(&para, &[], |t, level, _| recorder.shape(t, level)).unwrap();
        (
            recorder.seen.into_inner(),
            glyphs.iter().map(|g| g.cluster).collect(),
        )
    }

    /// A shaper that leaves a backward run in logical order must not be able to
    /// change what comes out: `cluster` reads the glyph order as the direction
    /// itself, so a run arriving the wrong way round used to fall apart into
    /// one run per glyph.
    #[test]
    fn a_backward_run_is_put_in_visual_order_whatever_the_shaper_did() {
        let para = Paragraph::new("a שלום", ParagraphDirection::Auto);
        let visual = Recorder::default();
        let logical = Recorder {
            logical_order: true,
            ..Default::default()
        };
        let of = |r: &Recorder| {
            shape_runs(&para, &[], |t, level, _| r.shape(t, level))
                .unwrap()
                .iter()
                .map(|g| g.cluster)
                .collect::<Vec<_>>()
        };
        assert_eq!(of(&visual), vec![0, 1, 5, 4, 3, 2]);
        assert_eq!(of(&logical), of(&visual));
    }

    /// The glyphs of one cluster keep their order, so a mark still follows the
    /// base it belongs to.
    #[test]
    fn reordering_a_backward_run_keeps_each_cluster_together_and_in_order() {
        let glyph = |cluster: usize, glyph_id: u16| ShapedGlyph {
            glyph_id,
            cluster,
            level: 1,
            x_advance: 0.0,
            y_advance: 0.0,
            x_offset: 0.0,
            y_offset: 0.0,
        };
        // Logical order, cluster 0 being a base plus two marks.
        let mut glyphs = vec![
            glyph(0, 10),
            glyph(0, 11),
            glyph(0, 12),
            glyph(1, 20),
            glyph(2, 30),
        ];
        to_visual_order(&mut glyphs, true);
        assert_eq!(
            glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            vec![30, 20, 10, 11, 12],
        );
        // Already visual: untouched.
        let before = glyphs.clone();
        to_visual_order(&mut glyphs, true);
        assert_eq!(
            glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            before.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
        );
        // A forward run is never reordered.
        let mut forward = vec![glyph(2, 30), glyph(0, 10)];
        to_visual_order(&mut forward, false);
        assert_eq!(
            forward.iter().map(|g| g.glyph_id).collect::<Vec<_>>(),
            vec![30, 10],
        );
    }

    #[test]
    fn single_script_text_is_shaped_in_one_call() {
        let (runs, clusters) = shape_recording("가나다");
        assert_eq!(runs, vec![("가나다".to_string(), 0)]);
        assert_eq!(clusters, vec![0, 1, 2]);
    }

    #[test]
    fn mixed_script_text_is_shaped_per_run() {
        let (runs, _) = shape_recording("ひ 각\u{302F}");
        assert_eq!(
            runs,
            vec![("ひ ".to_string(), 0), ("각\u{302F}".to_string(), 0)],
        );
    }

    #[test]
    fn clusters_are_rebased_onto_the_whole_text() {
        // Runs are "ひ " (chars 0..2) and "각\u{302F}" (chars 2..4).
        let (_, clusters) = shape_recording("ひ 각\u{302F}");
        assert_eq!(clusters, vec![0, 1, 2, 3]);
    }

    /// A right-to-left run's glyphs come out in visual order, so its cluster
    /// indices run backwards.
    #[test]
    fn a_right_to_left_run_comes_back_in_visual_order() {
        let (runs, clusters) = shape_recording("שלום");
        assert_eq!(runs, vec![("שלום".to_string(), 1)]);
        assert_eq!(clusters, vec![3, 2, 1, 0]);
    }

    /// The level runs themselves are visited in visual order, so the trailing
    /// Hebrew of an LTR paragraph is shaped last and painted last.
    #[test]
    fn level_runs_are_visited_in_visual_order() {
        let (runs, clusters) = shape_recording("a שלום");
        assert_eq!(runs, vec![("a ".to_string(), 0), ("שלום".to_string(), 1)]);
        assert_eq!(clusters, vec![0, 1, 5, 4, 3, 2]);
    }

    /// ...and in an RTL paragraph the *last* logical run is painted first.
    #[test]
    fn a_right_to_left_paragraph_paints_its_last_run_first() {
        let (runs, clusters) = shape_recording("שלום a");
        assert_eq!(runs, vec![("a".to_string(), 2), ("שלום ".to_string(), 1)]);
        assert_eq!(clusters, vec![5, 4, 3, 2, 1, 0]);
    }

    /// Inside one right-to-left level run, the script runs are visited back to
    /// front too — otherwise two scripts in one Hebrew phrase would come out
    /// with their halves swapped.
    #[test]
    fn script_runs_inside_a_backward_level_run_are_visited_back_to_front() {
        // Hebrew then Arabic, both RTL, one level run split by script.
        let (runs, _) = shape_recording("שלוםسلام");
        assert_eq!(runs, vec![("سلام".to_string(), 1), ("שלום".to_string(), 1)],);
    }

    #[test]
    fn an_explicit_paragraph_direction_reaches_the_runs() {
        let (runs, _) = shape_recording_dir("abc", ParagraphDirection::Rtl);
        assert_eq!(runs, vec![("abc".to_string(), 2)]);
    }

    #[test]
    fn features_are_clipped_and_rebased_per_run() {
        let feature = Feature {
            tag: *b"liga",
            value: 1,
            start: 1,
            end: 3,
        };
        // Whole-text range 1..3 covers the tail of run 0 and the head of run 1.
        assert_eq!(
            clip_feature(&feature, 0, 2).map(|f| (f.start, f.end)),
            Some((1, 2)),
        );
        assert_eq!(
            clip_feature(&feature, 2, 2).map(|f| (f.start, f.end)),
            Some((0, 1)),
        );
        // A run past the feature's range gets nothing.
        assert!(clip_feature(&feature, 4, 2).is_none());
    }

    #[test]
    fn whole_text_feature_range_survives_clipping() {
        let feature = Feature {
            tag: *b"kern",
            value: 1,
            start: 0,
            end: usize::MAX,
        };
        assert_eq!(
            clip_feature(&feature, 2, 2).map(|f| (f.start, f.end)),
            Some((0, 2)),
        );
    }

    #[test]
    fn char_levels_cover_every_character() {
        let para = Paragraph::new("a שלום", ParagraphDirection::Auto);
        assert_eq!(para.char_levels, vec![0, 0, 1, 1, 1, 1]);
        assert_eq!(para.level_of_char(2), 1);
        // Past the end takes the paragraph's own level rather than panicking;
        // a backend may report a cluster there.
        assert_eq!(para.level_of_char(99), 0);
    }
}
