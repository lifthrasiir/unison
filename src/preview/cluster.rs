//! Where a character sits on a shaped line, and which character a click hit.
//!
//! # Visual order, logical indices
//!
//! Glyphs arrive from a backend in *visual* order — the order they are painted,
//! left to right — while everything above (the caret, the selection, the text
//! model) counts in *logical* char indices. In a right-to-left run the two run
//! opposite ways, so this module is the one place that holds both: a
//! [`ClusterSpan`] is a horizontal stretch of the line, stored in visual order,
//! that knows the logical char range it stands for and which way round that
//! range is.
//!
//! # A run is where direction is constant
//!
//! Spans are grouped into runs by the glyphs' embedding level, and a run also
//! ends where the cluster indices stop moving the way the level says they
//! should — two runs of the same direction can be adjacent on screen without
//! being adjacent in the text, and merging them would give a span a char range
//! that spills across the gap. A run's logical extent is recovered by ordering
//! the runs' starts: they tile the paragraph, so each one reaches to the next.
//!
//! # Two caret positions at a boundary, and the affinity that picks one
//!
//! At a direction boundary one logical position genuinely has two screen
//! positions — the trailing edge of the left-to-right run and the "trailing"
//! edge of the right-to-left one — and nothing local to that position can
//! choose between them. Every stack that gets this right carries the choice as
//! a second piece of caret state: Gecko stores a bidi level on the caret
//! (`nsFrameSelection::mCaretBidiLevel`), DirectWrite makes it an argument
//! (`HitTestTextPosition`'s `isTrailingHit`), and Core Text hands back both
//! answers at once (`CTLineGetOffsetForStringIndex`'s `secondaryOffset`).
//!
//! So does this: a caret here is a `(char index, level)` pair — see
//! [`CaretPos`] — and the level says which run the caret belongs to. It is the
//! preview widget's state and not [`crate::editor::caret::Caret`]'s, because
//! that type is shared with the grid editor, which has no directions in it.
//!
//! # Movement is visual, selection is logical
//!
//! An arrow key moves the caret one step *on screen*, so it crosses a run
//! boundary by jumping in logical index; [`step`] is that walk. Extending a
//! selection stays logical, because the model holds a selection as one
//! `(anchor, cursor)` pair and a visually contiguous selection across a
//! boundary is not a contiguous logical range — it could not be represented.
//! That split is Firefox's default (`bidi.edit.caret_movement_style = 2`), and
//! for the same reason.

use crate::preview::ShapedGlyph;

/// One horizontal stretch of a shaped line, standing for a contiguous run of
/// characters. Held in visual order: `pen_x` increases along the vector.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterSpan {
    /// Logical char range, `char_start < char_end` whichever way the run runs.
    pub char_start: usize,
    pub char_end: usize,
    /// Left edge, in pixels from the start of the line.
    pub pen_x: f32,
    pub advance: f32,
    /// The embedding level of the run this span is in. Two spans can be
    /// adjacent on screen, run the same way and still belong to different runs
    /// — level 1 beside level 3 — so the caret's affinity is a level and not a
    /// direction.
    pub level: u8,
}

impl ClusterSpan {
    /// Whether the characters run right to left inside this span.
    pub fn rtl(&self) -> bool {
        self.level % 2 == 1
    }

    fn chars(&self) -> usize {
        (self.char_end - self.char_start).max(1)
    }

    /// The x of a boundary `fraction` of the way through the span's characters,
    /// counted the way the span runs.
    fn x_at(&self, fraction: f32) -> f32 {
        if self.rtl() {
            self.pen_x + self.advance * (1.0 - fraction)
        } else {
            self.pen_x + self.advance * fraction
        }
    }
}

/// One visual run: a stretch of glyphs at one level whose clusters move the
/// way that level says they should.
struct GlyphRun {
    glyphs: std::ops::Range<usize>,
    level: u8,
    /// The run's first character in logical order.
    char_start: usize,
}

/// Build the spans of one shaped line. `glyphs` must be in visual order, as
/// every [`crate::preview::ShaperBackend`] returns them.
pub fn build_clusters(glyphs: &[ShapedGlyph], px_size: f32, total_chars: usize) -> Vec<ClusterSpan> {
    let runs = split_glyph_runs(glyphs);
    let run_ends = logical_run_ends(&runs, total_chars);

    let mut spans: Vec<ClusterSpan> = Vec::new();
    let mut pen_x = 0.0f32;

    for (run, run_end) in runs.iter().zip(run_ends) {
        // The anchors this run's spans start at, so each can reach to the next
        // one up. Which visual neighbour that is depends on the direction, so
        // it is asked of the sorted set rather than of the neighbour.
        let mut anchors: Vec<usize> = Vec::new();
        let mut run_spans: Vec<ClusterSpan> = Vec::new();

        for g in &glyphs[run.glyphs.clone()] {
            let advance = g.x_advance * px_size;
            match run_spans.last_mut() {
                Some(last) if last.char_start == g.cluster => {
                    last.advance += advance;
                }
                _ => {
                    anchors.push(g.cluster);
                    run_spans.push(ClusterSpan {
                        char_start: g.cluster,
                        char_end: g.cluster + 1,
                        pen_x,
                        advance,
                        level: run.level,
                    });
                }
            }
            pen_x += advance;
        }

        anchors.sort_unstable();
        for span in &mut run_spans {
            span.char_end = anchors
                .iter()
                .copied()
                .find(|&a| a > span.char_start)
                .unwrap_or(run_end)
                .max(span.char_start + 1);
        }
        spans.append(&mut run_spans);
    }

    spans
}

/// Group glyphs into runs, breaking where the level changes and where the
/// cluster indices stop moving the way the level says they should.
fn split_glyph_runs(glyphs: &[ShapedGlyph]) -> Vec<GlyphRun> {
    let mut runs: Vec<GlyphRun> = Vec::new();
    for (i, g) in glyphs.iter().enumerate() {
        let rtl = g.level % 2 == 1;
        let continues = runs.last().is_some_and(|run| {
            run.level == g.level && {
                let prev = glyphs[run.glyphs.end - 1].cluster;
                if rtl {
                    g.cluster <= prev
                } else {
                    g.cluster >= prev
                }
            }
        });
        match runs.last_mut() {
            Some(run) if continues => run.glyphs.end = i + 1,
            _ => runs.push(GlyphRun {
                glyphs: i..i + 1,
                level: g.level,
                char_start: g.cluster,
            }),
        }
    }
    // A right-to-left run's first *logical* character is its last visual one.
    for run in &mut runs {
        run.char_start = glyphs[run.glyphs.clone()]
            .iter()
            .map(|g| g.cluster)
            .min()
            .unwrap_or(0);
    }
    runs
}

/// The logical char each run reaches to. The runs tile the paragraph, so a run
/// ends where the next one along in *logical* order starts.
fn logical_run_ends(runs: &[GlyphRun], total_chars: usize) -> Vec<usize> {
    let mut starts: Vec<usize> = runs.iter().map(|r| r.char_start).collect();
    starts.sort_unstable();
    runs.iter()
        .map(|run| {
            starts
                .iter()
                .copied()
                .find(|&s| s > run.char_start)
                .unwrap_or(total_chars)
                .max(run.char_start + 1)
        })
        .collect()
}

/// Where a caret is: a logical position, plus the run it belongs to.
///
/// The level is the *affinity* — at a direction boundary two runs meet at one
/// logical position and it says which of the two screen positions is meant.
/// A caret that has never been placed against a run carries the paragraph's own
/// level, which is what a fresh, wholly one-directional line gives it anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretPos {
    pub char_idx: usize,
    pub level: u8,
}

impl CaretPos {
    pub fn new(char_idx: usize, level: u8) -> Self {
        Self { char_idx, level }
    }
}

/// A caret resolved onto the line: which span it is in, and how many character
/// boundaries into that span it sits, counted *visually* from the span's left
/// edge. `v` ranges over `0..=chars`, so both edges are expressible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resolved {
    span: usize,
    v: usize,
}

impl Resolved {
    fn to_caret(self, clusters: &[ClusterSpan]) -> CaretPos {
        let span = &clusters[self.span];
        let n = span.chars();
        let offset = if span.rtl() { n - self.v } else { self.v };
        CaretPos::new(span.char_start + offset, span.level)
    }

    /// `v` is already counted from the span's left edge, so this is a plain
    /// interpolation — unlike [`ClusterSpan::x_at`], which is handed a fraction
    /// counted the way the *characters* run and has to flip it.
    fn x(self, clusters: &[ClusterSpan]) -> f32 {
        let span = &clusters[self.span];
        span.pen_x + span.advance * self.v as f32 / span.chars() as f32
    }
}

/// Resolve a caret onto the line's spans.
///
/// A boundary is shared by two spans, so more than one can answer; the level is
/// what breaks the tie, and after that a span the position falls *inside* is
/// preferred over one it merely touches, so that stepping through a run does
/// not keep hopping to its neighbour.
fn resolve(clusters: &[ClusterSpan], caret: CaretPos) -> Option<Resolved> {
    let candidates = |level_matches: bool, interior: bool| {
        clusters.iter().enumerate().find(|(_, span)| {
            (span.level == caret.level) == level_matches
                && caret.char_idx >= span.char_start
                && if interior {
                    caret.char_idx < span.char_end
                } else {
                    caret.char_idx <= span.char_end
                }
        })
    };
    let (idx, span) = candidates(true, true)
        .or_else(|| candidates(true, false))
        .or_else(|| candidates(false, true))
        .or_else(|| candidates(false, false))?;
    let n = span.chars();
    let offset = (caret.char_idx - span.char_start).min(n);
    Some(Resolved {
        span: idx,
        v: if span.rtl() { n - offset } else { offset },
    })
}

/// Which way an arrow key moves on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Left,
    Right,
}

/// Move `caret` one character boundary along the screen, or `None` if that
/// would leave the line — which is the caller's cue to move to the line beside
/// it rather than to stay put.
///
/// The step is visual, so in a right-to-left run [`Step::Right`] *decreases*
/// the logical index, and at a run boundary the index jumps by however far the
/// two runs are apart in the text.
pub fn step(clusters: &[ClusterSpan], caret: CaretPos, dir: Step) -> Option<CaretPos> {
    let here = resolve(clusters, caret)?;
    let n = clusters[here.span].chars();
    let next = match dir {
        Step::Right if here.v < n => Resolved {
            span: here.span,
            v: here.v + 1,
        },
        // At the span's right edge the caret is already on the seam with the
        // span to its right, so the step lands one character *into* that one.
        Step::Right => Resolved {
            span: here.span.checked_add(1).filter(|&s| s < clusters.len())?,
            v: 1,
        },
        Step::Left if here.v > 0 => Resolved {
            span: here.span,
            v: here.v - 1,
        },
        Step::Left => {
            let span = here.span.checked_sub(1)?;
            Resolved {
                span,
                v: clusters[span].chars() - 1,
            }
        }
    };
    Some(next.to_caret(clusters))
}

/// Where a caret entering the line from the side lands: the outermost boundary
/// on that edge. Used when an arrow key walks off one line onto the next.
pub fn edge_caret(clusters: &[ClusterSpan], side: Step) -> Option<CaretPos> {
    let (idx, span) = match side {
        Step::Left => (0, clusters.first()?),
        Step::Right => (clusters.len() - 1, clusters.last()?),
    };
    Some(
        Resolved {
            span: idx,
            v: match side {
                Step::Left => 0,
                Step::Right => span.chars(),
            },
        }
        .to_caret(clusters),
    )
}

/// The x of a caret, honouring its affinity.
pub fn caret_pos_x(clusters: &[ClusterSpan], caret: CaretPos) -> f32 {
    match resolve(clusters, caret) {
        Some(r) => r.x(clusters),
        None => caret_x(clusters, caret.char_idx),
    }
}

/// The x of a caret at logical position `char_idx`.
pub fn caret_x(clusters: &[ClusterSpan], char_idx: usize) -> f32 {
    if clusters.is_empty() {
        return 0.0;
    }
    if let Some(x) = edge_x(clusters, char_idx, true) {
        return x;
    }
    // Past the last character: the trailing edge of the one before it.
    if let Some(x) = char_idx
        .checked_sub(1)
        .and_then(|prev| edge_x(clusters, prev, false))
    {
        return x;
    }
    clusters.first().map_or(0.0, |s| s.x_at(0.0))
}

/// The x of one edge of the character at `char_idx`, or `None` if no span
/// covers it.
fn edge_x(clusters: &[ClusterSpan], char_idx: usize, leading: bool) -> Option<f32> {
    let span = clusters
        .iter()
        .find(|s| char_idx >= s.char_start && char_idx < s.char_end)?;
    let offset = char_idx - span.char_start + usize::from(!leading);
    Some(span.x_at(offset as f32 / span.chars() as f32))
}

/// The level of the run the character at `char_idx` sits in, or `None` past
/// the end of the line — where the paragraph's own level is the answer.
pub fn level_at(clusters: &[ClusterSpan], char_idx: usize) -> Option<u8> {
    clusters
        .iter()
        .find(|s| char_idx >= s.char_start && char_idx < s.char_end)
        .map(|s| s.level)
}

/// Which caret a click at `x` lands on, affinity included: clicking inside a
/// run is how the caret is told which run it belongs to. `paragraph_level`
/// answers for an empty line, which has no span to ask.
pub fn caret_from_x(clusters: &[ClusterSpan], x: f32, paragraph_level: u8) -> CaretPos {
    let level = span_at_x(clusters, x).map_or(paragraph_level, |s| s.level);
    CaretPos::new(char_idx_from_x(clusters, x), level)
}

/// The span `x` falls in — the nearer end span when it is past either end.
fn span_at_x(clusters: &[ClusterSpan], x: f32) -> Option<&ClusterSpan> {
    let first = clusters.first()?;
    if x < first.pen_x {
        return Some(first);
    }
    clusters
        .iter()
        .find(|s| x < s.pen_x + s.advance)
        .or_else(|| clusters.last())
}

/// Which logical position a click at `x` lands on.
pub fn char_idx_from_x(clusters: &[ClusterSpan], x: f32) -> usize {
    let Some(first) = clusters.first() else {
        return 0;
    };
    let last = clusters.last().expect("non-empty");

    let span = span_at_x(clusters, x).unwrap_or(last);
    if x < first.pen_x {
        return outer_edge(first, true);
    }
    if x >= last.pen_x + last.advance {
        return outer_edge(last, false);
    }

    let chars = span.chars();
    let mut fraction = ((x - span.pen_x) / span.advance.max(f32::EPSILON)).clamp(0.0, 1.0);
    if span.rtl() {
        fraction = 1.0 - fraction;
    }
    span.char_start + ((fraction * chars as f32).round() as usize).min(chars)
}

/// The logical position at the left (`left`) or right edge of an end span.
fn outer_edge(span: &ClusterSpan, left: bool) -> usize {
    if left == span.rtl() {
        span.char_end
    } else {
        span.char_start
    }
}

/// The horizontal stretches a logical selection of `lo..hi` covers. One span
/// per stretch, in visual order, with touching stretches merged — a selection
/// that crosses a direction boundary is genuinely discontiguous on screen.
pub fn selection_rects(clusters: &[ClusterSpan], lo: usize, hi: usize) -> Vec<(f32, f32)> {
    let mut rects: Vec<(f32, f32)> = Vec::new();
    for span in clusters {
        let from = span.char_start.max(lo);
        let to = span.char_end.min(hi);
        if from >= to {
            continue;
        }
        let chars = span.chars() as f32;
        let a = span.x_at((from - span.char_start) as f32 / chars);
        let b = span.x_at((to - span.char_start) as f32 / chars);
        let (x0, x1) = if a <= b { (a, b) } else { (b, a) };
        match rects.last_mut() {
            Some(last) if x0 <= last.1 + 0.01 => last.1 = last.1.max(x1),
            _ => rects.push((x0, x1)),
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(glyph_id, cluster, level, advance)`.
    fn make_glyphs(specs: &[(u16, usize, u8, f32)]) -> Vec<ShapedGlyph> {
        specs
            .iter()
            .map(|&(glyph_id, cluster, level, x_advance)| ShapedGlyph {
                glyph_id,
                cluster,
                level,
                x_advance,
                y_advance: 0.0,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect()
    }

    /// Three left-to-right characters, 10px each.
    fn ltr() -> Vec<ClusterSpan> {
        let glyphs = make_glyphs(&[(1, 0, 0, 0.5), (2, 1, 0, 0.5), (3, 2, 0, 0.5)]);
        build_clusters(&glyphs, 20.0, 3)
    }

    /// Three right-to-left characters, 10px each: char 2 is leftmost.
    fn rtl() -> Vec<ClusterSpan> {
        let glyphs = make_glyphs(&[(3, 2, 1, 0.5), (2, 1, 1, 0.5), (1, 0, 1, 0.5)]);
        build_clusters(&glyphs, 20.0, 3)
    }

    #[test]
    fn simple_ascii() {
        let clusters = ltr();
        assert_eq!(clusters.len(), 3);
        assert!((caret_x(&clusters, 0) - 0.0).abs() < 0.01);
        assert!((caret_x(&clusters, 1) - 10.0).abs() < 0.01);
        assert!((caret_x(&clusters, 2) - 20.0).abs() < 0.01);
        assert!((caret_x(&clusters, 3) - 30.0).abs() < 0.01);
    }

    #[test]
    fn ligature_cluster() {
        // "ffi": 3 chars → 1 glyph of 0.6em.
        let glyphs = make_glyphs(&[(10, 0, 0, 0.6)]);
        let clusters = build_clusters(&glyphs, 20.0, 3);
        assert_eq!(clusters.len(), 1);
        assert_eq!((clusters[0].char_start, clusters[0].char_end), (0, 3));
        assert!((caret_x(&clusters, 1) - 4.0).abs() < 0.01);
        assert!((caret_x(&clusters, 2) - 8.0).abs() < 0.01);
    }

    /// A mark adds no advance of its own, so it joins its base's span.
    #[test]
    fn a_zero_advance_glyph_joins_its_base_cluster() {
        let glyphs = make_glyphs(&[(1, 0, 0, 0.5), (2, 0, 0, 0.0), (3, 1, 0, 0.5)]);
        let clusters = build_clusters(&glyphs, 20.0, 2);
        assert_eq!(clusters.len(), 2);
        assert!((clusters[0].advance - 10.0).abs() < 0.01);
    }

    /// A mark the shaper gave a cluster of its own becomes a span with no
    /// width, so an arrow key over it advances the caret in the text without
    /// moving it on screen. Recorded rather than worked around: the same
    /// character is invisible in the grid editor too, and the two want one
    /// answer — see the combining-mark display work, not this module.
    #[test]
    fn a_mark_in_a_cluster_of_its_own_is_a_span_with_no_width() {
        let glyphs = make_glyphs(&[(1, 0, 0, 0.5), (2, 1, 0, 0.0), (3, 2, 0, 0.5)]);
        let clusters = build_clusters(&glyphs, 20.0, 3);
        assert_eq!(clusters.len(), 3);
        assert_eq!(clusters[1].advance, 0.0);
        // The step is taken — the caret is at a different character — but the
        // two positions are the same point on screen.
        let a = CaretPos::new(1, 0);
        let b = step(&clusters, a, Step::Right).unwrap();
        assert_eq!(b.char_idx, 2);
        assert_eq!(caret_pos_x(&clusters, a), caret_pos_x(&clusters, b));
    }

    #[test]
    fn click_to_char() {
        let clusters = ltr();
        assert_eq!(char_idx_from_x(&clusters, 0.0), 0);
        assert_eq!(char_idx_from_x(&clusters, 5.0), 1);
        assert_eq!(char_idx_from_x(&clusters, 4.9), 0);
        assert_eq!(char_idx_from_x(&clusters, 35.0), 3);
    }

    /// The whole point: char 0 of a right-to-left run is the *rightmost* one.
    #[test]
    fn a_right_to_left_run_reads_right_to_left() {
        let clusters = rtl();
        assert_eq!(clusters.len(), 3);
        assert_eq!(
            clusters.iter().map(|c| c.char_start).collect::<Vec<_>>(),
            vec![2, 1, 0],
        );
        assert!(clusters.iter().all(|c| c.rtl()));
        // Caret before char 0 is at the right edge; past the last char, left.
        assert!((caret_x(&clusters, 0) - 30.0).abs() < 0.01);
        assert!((caret_x(&clusters, 1) - 20.0).abs() < 0.01);
        assert!((caret_x(&clusters, 3) - 0.0).abs() < 0.01);
    }

    #[test]
    fn clicking_a_right_to_left_run_picks_the_character_under_the_pointer() {
        let clusters = rtl();
        // The leftmost cell is char 2; its right edge is position 2.
        assert_eq!(char_idx_from_x(&clusters, 1.0), 3);
        assert_eq!(char_idx_from_x(&clusters, 9.0), 2);
        assert_eq!(char_idx_from_x(&clusters, 29.0), 0);
        // Past either end of the line.
        assert_eq!(char_idx_from_x(&clusters, -5.0), 3);
        assert_eq!(char_idx_from_x(&clusters, 99.0), 0);
    }

    /// `a` then two right-to-left characters: the runs are adjacent on screen
    /// and the second one's characters count backwards.
    fn mixed() -> Vec<ClusterSpan> {
        let glyphs = make_glyphs(&[(1, 0, 0, 0.5), (3, 2, 1, 0.5), (2, 1, 1, 0.5)]);
        build_clusters(&glyphs, 20.0, 3)
    }

    #[test]
    fn a_level_change_starts_a_new_run() {
        let clusters = mixed();
        assert_eq!(
            clusters
                .iter()
                .map(|c| (c.char_start, c.char_end, c.rtl()))
                .collect::<Vec<_>>(),
            vec![(0, 1, false), (2, 3, true), (1, 2, true)],
        );
    }

    /// The caret at position 1 is a direction boundary: the rule is the leading
    /// edge of the character *at* 1, which is inside the right-to-left run.
    #[test]
    fn a_caret_at_a_direction_boundary_takes_the_leading_edge() {
        let clusters = mixed();
        assert!((caret_x(&clusters, 1) - 30.0).abs() < 0.01);
    }

    /// Two right-to-left runs that are adjacent on screen but not in the text
    /// must not merge, or the span between them would claim characters it does
    /// not cover.
    #[test]
    fn two_backward_runs_at_different_levels_stay_apart() {
        let glyphs = make_glyphs(&[(1, 3, 1, 0.5), (2, 0, 3, 0.5)]);
        let clusters = build_clusters(&glyphs, 20.0, 4);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].char_start, 3);
        assert_eq!(clusters[1].char_start, 0);
    }

    #[test]
    fn a_selection_inside_one_run_is_one_rect() {
        let clusters = ltr();
        let rects = selection_rects(&clusters, 0, 2);
        assert_eq!(rects.len(), 1);
        assert!((rects[0].0 - 0.0).abs() < 0.01);
        assert!((rects[0].1 - 20.0).abs() < 0.01);
    }

    /// A logically contiguous selection across a direction boundary is two
    /// stretches on screen, and they are not the two halves you would guess.
    #[test]
    fn a_selection_across_a_direction_boundary_is_split() {
        let clusters = mixed();
        let rects = selection_rects(&clusters, 0, 2);
        assert_eq!(rects.len(), 2);
        assert!((rects[0].0 - 0.0).abs() < 0.01);
        assert!((rects[0].1 - 10.0).abs() < 0.01);
        assert!((rects[1].0 - 20.0).abs() < 0.01);
        assert!((rects[1].1 - 30.0).abs() < 0.01);
    }

    /// `a` at 0, then Hebrew chars 1..4, then `b` at 4: the shape of every
    /// example in the bidi literature, and the one the caret rules are about.
    ///
    /// Logical: a ש ל ו b        Visual: a ו ל ש b
    /// index:   0 1 2 3 4                0 3 2 1 4
    fn embedded() -> Vec<ClusterSpan> {
        let glyphs = make_glyphs(&[
            (1, 0, 0, 0.5),
            (4, 3, 1, 0.5),
            (3, 2, 1, 0.5),
            (2, 1, 1, 0.5),
            (5, 4, 0, 0.5),
        ]);
        build_clusters(&glyphs, 20.0, 5)
    }

    /// Walking right with an arrow key visits the line left to right, whatever
    /// the logical indices do — and here they run 0, 1, 2, 3, 4 only because
    /// each step lands on the *leading* edge of the next character along.
    #[test]
    fn stepping_right_walks_the_line_left_to_right() {
        let clusters = embedded();
        let mut caret = CaretPos::new(0, 0);
        let mut xs = vec![caret_pos_x(&clusters, caret)];
        while let Some(next) = step(&clusters, caret, Step::Right) {
            caret = next;
            xs.push(caret_pos_x(&clusters, caret));
        }
        assert_eq!(xs, vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    }

    #[test]
    fn stepping_left_walks_the_line_right_to_left() {
        let clusters = embedded();
        let mut caret = CaretPos::new(5, 0);
        let mut xs = vec![caret_pos_x(&clusters, caret)];
        while let Some(next) = step(&clusters, caret, Step::Left) {
            caret = next;
            xs.push(caret_pos_x(&clusters, caret));
        }
        assert_eq!(xs, vec![50.0, 40.0, 30.0, 20.0, 10.0, 0.0]);
    }

    /// Inside the right-to-left run a rightward step *decreases* the logical
    /// index — the point of visual movement.
    #[test]
    fn a_rightward_step_inside_a_backward_run_moves_back_in_the_text() {
        let clusters = embedded();
        // Caret between ל and ו (logical 3, inside the RTL run).
        let caret = CaretPos::new(3, 1);
        let next = step(&clusters, caret, Step::Right).unwrap();
        assert_eq!(next, CaretPos::new(2, 1));
    }

    /// The two carets at the boundary: same logical position, two screen
    /// positions, told apart only by the affinity.
    #[test]
    fn one_logical_position_has_two_screen_positions_at_a_boundary() {
        let clusters = embedded();
        // Logical 1 is where `a` ends and the Hebrew begins.
        assert!((caret_pos_x(&clusters, CaretPos::new(1, 0)) - 10.0).abs() < 0.01);
        assert!((caret_pos_x(&clusters, CaretPos::new(1, 1)) - 40.0).abs() < 0.01);
    }

    /// Stepping out of the left-to-right run and into the right-to-left one
    /// lands with the affinity of the run entered, not the one left.
    #[test]
    fn a_step_across_a_boundary_takes_the_affinity_of_the_run_entered() {
        let clusters = embedded();
        let caret = step(&clusters, CaretPos::new(0, 0), Step::Right).unwrap();
        assert_eq!(caret, CaretPos::new(1, 0));
        // ...and one more lands inside the Hebrew, at its rightmost boundary.
        let caret = step(&clusters, caret, Step::Right).unwrap();
        assert_eq!(caret.level, 1);
        assert!((caret_pos_x(&clusters, caret) - 20.0).abs() < 0.01);
    }

    /// Walking off either end is `None`, so the caller can move to the line
    /// beside this one instead of pinning the caret to the edge.
    #[test]
    fn a_step_off_the_end_of_the_line_is_refused() {
        let clusters = embedded();
        assert!(step(&clusters, CaretPos::new(5, 0), Step::Right).is_none());
        assert!(step(&clusters, CaretPos::new(0, 0), Step::Left).is_none());
    }

    /// Entering a line from the side lands on the outermost boundary, which in
    /// a right-to-left line is *not* logical position 0 or the end.
    #[test]
    fn entering_a_line_from_the_side_lands_on_its_outer_edge() {
        let ltr = ltr();
        assert_eq!(edge_caret(&ltr, Step::Left).unwrap().char_idx, 0);
        assert_eq!(edge_caret(&ltr, Step::Right).unwrap().char_idx, 3);
        let rtl = rtl();
        assert_eq!(edge_caret(&rtl, Step::Left).unwrap().char_idx, 3);
        assert_eq!(edge_caret(&rtl, Step::Right).unwrap().char_idx, 0);
        assert!(edge_caret(&[], Step::Left).is_none());
    }

    /// A click is where the caret's affinity comes from when it is not moving.
    #[test]
    fn a_click_reports_the_run_it_landed_in() {
        let clusters = embedded();
        assert_eq!(caret_from_x(&clusters, 5.0, 0).level, 0);
        assert_eq!(caret_from_x(&clusters, 25.0, 0).level, 1);
        assert_eq!(caret_from_x(&clusters, 45.0, 0).level, 0);
        // An empty line has nothing to ask, so the paragraph answers.
        assert_eq!(caret_from_x(&[], 5.0, 1).level, 1);
    }

    #[test]
    fn an_empty_line_has_no_spans() {
        let clusters = build_clusters(&[], 20.0, 0);
        assert!(clusters.is_empty());
        assert_eq!(caret_x(&clusters, 0), 0.0);
        assert_eq!(char_idx_from_x(&clusters, 10.0), 0);
    }
}

