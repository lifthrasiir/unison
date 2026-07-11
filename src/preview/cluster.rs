use crate::preview::ShapedGlyph;

#[derive(Clone, Debug)]
pub struct ClusterSpan {
    pub char_start: usize,
    pub char_end: usize,
    pub pen_x: f32,
    pub advance: f32,
}

pub fn build_clusters(glyphs: &[ShapedGlyph], px_size: f32) -> Vec<ClusterSpan> {
    if glyphs.is_empty() {
        return Vec::new();
    }

    let mut clusters: Vec<ClusterSpan> = Vec::new();
    let mut pen_x: f32 = 0.0;

    for g in glyphs {
        let advance = g.x_advance * px_size;
        if let Some(last) = clusters.last_mut() {
            if g.cluster == last.char_start {
                last.advance += advance;
                pen_x += advance;
                continue;
            }
            // new cluster: set char_end of previous to this cluster's char index
            last.char_end = g.cluster;
        }
        clusters.push(ClusterSpan {
            char_start: g.cluster,
            char_end: g.cluster + 1, // placeholder, updated by next cluster or finalize
            pen_x,
            advance,
        });
        pen_x += advance;
    }

    clusters
}

pub fn finalize_clusters(clusters: &mut [ClusterSpan], total_chars: usize) {
    if let Some(last) = clusters.last_mut() {
        if last.char_end <= last.char_start {
            last.char_end = total_chars;
        }
        // also fix any cluster whose char_end wasn't properly set
        if last.char_end < total_chars {
            last.char_end = total_chars;
        }
    }
}

pub fn caret_x(clusters: &[ClusterSpan], char_idx: usize) -> f32 {
    if clusters.is_empty() {
        return 0.0;
    }

    for span in clusters {
        if char_idx < span.char_start {
            return span.pen_x;
        }
        if char_idx < span.char_end {
            let span_chars = (span.char_end - span.char_start).max(1) as f32;
            let fraction = (char_idx - span.char_start) as f32 / span_chars;
            return span.pen_x + fraction * span.advance;
        }
    }

    // past end
    if let Some(last) = clusters.last() {
        last.pen_x + last.advance
    } else {
        0.0
    }
}

pub fn char_idx_from_x(clusters: &[ClusterSpan], x: f32) -> usize {
    if clusters.is_empty() {
        return 0;
    }

    for span in clusters {
        if x < span.pen_x {
            return span.char_start;
        }
        let end_x = span.pen_x + span.advance;
        if x < end_x {
            let span_chars = (span.char_end - span.char_start).max(1);
            let frac = (x - span.pen_x) / span.advance;
            let offset = (frac * span_chars as f32).round() as usize;
            return span.char_start + offset.min(span_chars);
        }
    }

    clusters.last().map_or(0, |s| s.char_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_glyphs(specs: &[(u16, usize, f32)]) -> Vec<ShapedGlyph> {
        specs
            .iter()
            .map(|&(glyph_id, cluster, x_advance)| ShapedGlyph {
                glyph_id,
                cluster,
                x_advance,
                y_advance: 0.0,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect()
    }

    #[test]
    fn simple_ascii() {
        // 3 glyphs, each 0.5 em, at px_size=20 → 10px each
        let glyphs = make_glyphs(&[(1, 0, 0.5), (2, 1, 0.5), (3, 2, 0.5)]);
        let mut clusters = build_clusters(&glyphs, 20.0);
        finalize_clusters(&mut clusters, 3);

        assert_eq!(clusters.len(), 3);
        assert!((caret_x(&clusters, 0) - 0.0).abs() < 0.01);
        assert!((caret_x(&clusters, 1) - 10.0).abs() < 0.01);
        assert!((caret_x(&clusters, 2) - 20.0).abs() < 0.01);
        assert!((caret_x(&clusters, 3) - 30.0).abs() < 0.01);
    }

    #[test]
    fn ligature_cluster() {
        // "ffi" ligature: 3 chars → 1 glyph with advance 0.6em
        let glyphs = make_glyphs(&[(10, 0, 0.6)]);
        let mut clusters = build_clusters(&glyphs, 20.0);
        finalize_clusters(&mut clusters, 3);

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].char_start, 0);
        assert_eq!(clusters[0].char_end, 3);
        // caret at char 1 should be 1/3 of the way through
        assert!((caret_x(&clusters, 1) - 4.0).abs() < 0.01);
        assert!((caret_x(&clusters, 2) - 8.0).abs() < 0.01);
    }

    #[test]
    fn click_to_char() {
        let glyphs = make_glyphs(&[(1, 0, 0.5), (2, 1, 0.5), (3, 2, 0.5)]);
        let mut clusters = build_clusters(&glyphs, 20.0);
        finalize_clusters(&mut clusters, 3);

        assert_eq!(char_idx_from_x(&clusters, 0.0), 0);
        assert_eq!(char_idx_from_x(&clusters, 5.0), 1); // middle of first glyph → round to 1
        assert_eq!(char_idx_from_x(&clusters, 4.9), 0); // just before middle → round to 0
        assert_eq!(char_idx_from_x(&clusters, 35.0), 3); // past end
    }
}
