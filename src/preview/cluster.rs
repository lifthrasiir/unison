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
//! # Two caret positions at a boundary
//!
//! At a direction boundary one logical position has two legitimate screen
//! positions, and no local rule can pick between them. The rule taken here is
//! the simple one: a caret at logical position `p` sits at the *leading* edge
//! of the character at `p` — its right edge inside a right-to-left run — and
//! only past the last character does it fall back to the trailing edge of the
//! one before. It is predictable, which matters more here than being clever.

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
    /// Whether the characters run right to left inside this span.
    pub rtl: bool,
}

impl ClusterSpan {
    fn chars(&self) -> usize {
        (self.char_end - self.char_start).max(1)
    }

    /// The x of a boundary `fraction` of the way through the span's characters,
    /// counted the way the span runs.
    fn x_at(&self, fraction: f32) -> f32 {
        if self.rtl {
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
    rtl: bool,
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
                        rtl: run.rtl,
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
            run.rtl == rtl
                && glyphs[run.glyphs.end - 1].level == g.level
                && {
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
                rtl,
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

/// Which logical position a click at `x` lands on.
pub fn char_idx_from_x(clusters: &[ClusterSpan], x: f32) -> usize {
    let Some(first) = clusters.first() else {
        return 0;
    };
    let last = clusters.last().expect("non-empty");

    let span = clusters
        .iter()
        .find(|s| x < s.pen_x + s.advance)
        .unwrap_or(last);
    if x < first.pen_x {
        return outer_edge(first, true);
    }
    if x >= last.pen_x + last.advance {
        return outer_edge(last, false);
    }

    let chars = span.chars();
    let mut fraction = ((x - span.pen_x) / span.advance.max(f32::EPSILON)).clamp(0.0, 1.0);
    if span.rtl {
        fraction = 1.0 - fraction;
    }
    span.char_start + ((fraction * chars as f32).round() as usize).min(chars)
}

/// The logical position at the left (`left`) or right edge of an end span.
fn outer_edge(span: &ClusterSpan, left: bool) -> usize {
    if left == span.rtl {
        span.char_end
    } else {
        span.char_start
    }
}

/// The horizontal stretches a logical selection of `lo..hi` covers. One span
/// per stretch, in visual order, with touching stretches merged — a selection
/// that crosses a direction boundary is genuinely discontiguous on screen.
// Waiting on the preview's selection painting, which still draws one rect.
#[cfg_attr(not(test), expect(dead_code))]
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
        assert!(clusters.iter().all(|c| c.rtl));
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
                .map(|c| (c.char_start, c.char_end, c.rtl))
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

    #[test]
    fn an_empty_line_has_no_spans() {
        let clusters = build_clusters(&[], 20.0, 0);
        assert!(clusters.is_empty());
        assert_eq!(caret_x(&clusters, 0), 0.0);
        assert_eq!(char_idx_from_x(&clusters, 10.0), 0);
    }
}
