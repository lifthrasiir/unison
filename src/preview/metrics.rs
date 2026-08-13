//! The vertical extents a preview row occupies, read from the face being
//! previewed rather than assumed from the font size.
//!
//! The widget paints chrome around each shaped run — a selection band, the IME
//! preedit's box, the caret — and every one of those is a rectangle measured
//! from the baseline. The preedit box is the one that has to be *exactly*
//! right: the glyphs inside it are painted in the widget's background colour,
//! inverse-video style, so whatever the box fails to cover is drawn as
//! background on background and simply disappears. A box that reached a fixed
//! `4.0` points below the baseline therefore erased the bottom of every
//! descending glyph as soon as the font size grew past 16 — the depth of a
//! descender scales with the size, the constant did not.
//!
//! The extents span the face's alignment box *and* its glyph bounding box.
//! Ascent and descent alone do not bound the ink: a face may draw past them
//! (Unison's Ogham reaches a pixel above its ascent), and a pixel the box does
//! not cover is a pixel the reader never sees.
//!
//! Everything here is in ems, so a value survives a change of font size; the
//! widget multiplies by the size it is drawing at.

/// How far a row reaches above and below its baseline, in ems.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VMetrics {
    /// Distance from the baseline up to the top of the row.
    pub ascent: f32,
    /// Distance from the baseline down to the bottom of the row, positive
    /// downwards — unlike `hhea.descender`, which is negative.
    pub descent: f32,
}

impl Default for VMetrics {
    /// What the widget assumed before it read any face: a full em above the
    /// baseline and a quarter em below.
    fn default() -> Self {
        Self {
            ascent: 1.0,
            descent: 0.25,
        }
    }
}

impl VMetrics {
    /// Reads the extents of the face in `font_data`, falling back to
    /// [`VMetrics::default`] for anything that cannot be parsed — the preview
    /// draws whatever the build produced, including a font a broken source
    /// left unreadable, and a missing metric is no reason to paint nothing.
    pub fn read(font_data: &[u8]) -> Self {
        use skrifa::{FontRef, MetadataProvider, prelude::*};

        let Ok(font) = FontRef::new(font_data) else {
            return Self::default();
        };
        let m = font.metrics(Size::unscaled(), LocationRef::default());
        if m.units_per_em == 0 {
            return Self::default();
        }
        let upem = f32::from(m.units_per_em);
        // `hhea.descender` points down as a negative number; ours points down
        // as a positive one, so that `above`/`below` read the same way.
        let (mut above, mut below) = (m.ascent, -m.descent);
        if let Some(bounds) = m.bounds {
            above = above.max(bounds.y_max);
            below = below.max(-bounds.y_min);
        }
        let read = Self {
            ascent: above / upem,
            descent: below / upem,
        };
        if read.ascent + read.descent > 0.0 {
            read
        } else {
            Self::default()
        }
    }

    /// Distance from the baseline to the top of a row drawn at `px_size`.
    pub fn above(&self, px_size: f32) -> f32 {
        self.ascent * px_size
    }

    /// Distance from the baseline to the bottom of a row drawn at `px_size`.
    pub fn below(&self, px_size: f32) -> f32 {
        self.descent * px_size
    }

    /// Baseline-to-baseline distance at `px_size`.
    ///
    /// The rhythm is the widget's own — a stable, size-proportional step keeps
    /// caret placement and hit testing simple to reason about — but it can
    /// never be tighter than the face needs, or consecutive rows would overlap.
    pub fn line_height(&self, px_size: f32) -> f32 {
        (px_size * 1.4)
            .round()
            .max(px_size + 4.0)
            .max(self.above(px_size) + self.below(px_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unison's own metrics: a 16-pixel em with `meta ascent 13`/`descent 3`.
    const UNISON: VMetrics = VMetrics {
        ascent: 13.0 / 16.0,
        descent: 3.0 / 16.0,
    };

    /// The sizes the preview's slider snaps to (`preview_font_size_slider`).
    const SIZES: [f32; 5] = [16.0, 32.0, 48.0, 64.0, 128.0];

    /// The preedit box hides any glyph it does not cover, so it has to reach
    /// as deep as the face draws — at every size, not just at 16, where a
    /// three-pixel descender happened to fit under a hardcoded `4.0`.
    #[test]
    fn row_covers_the_faces_ink_at_every_size() {
        for px in SIZES {
            let em = px / 16.0;
            assert!(
                UNISON.below(px) >= 3.0 * em,
                "at {px}px a descender reaches {} below the baseline, box reaches {}",
                3.0 * em,
                UNISON.below(px),
            );
            assert!(
                UNISON.above(px) >= 13.0 * em,
                "at {px}px an ascender reaches {} above the baseline, box reaches {}",
                13.0 * em,
                UNISON.above(px),
            );
        }
    }

    /// A face whose ink runs deeper than a quarter em is the whole point of
    /// reading the metrics, so the extents must follow the face and not the
    /// default.
    #[test]
    fn row_follows_the_face_rather_than_a_constant() {
        let deep = VMetrics {
            ascent: 0.75,
            descent: 0.5,
        };
        assert_eq!(deep.below(64.0), 32.0);
        assert_eq!(deep.above(64.0), 48.0);
    }

    /// Built from a source, not from `font/`: the point is that whatever a
    /// source declares is what the preview lays out with, so the fixture
    /// states its own metrics.
    fn font_from(source: &str) -> Vec<u8> {
        let doc = crate::document_io::parse_document_from_str(source, "test.unf".into()).unwrap();
        crate::render::ttf_builder::build_font_from_documents(&[&doc]).expect("font should build")
    }

    /// The declared alignment box reaches the widget intact — this is the
    /// whole wiring, from a `meta` line to the rectangle a row is drawn in.
    #[test]
    fn read_takes_the_declared_alignment_box() {
        let font = font_from(
            "\
meta height 4
meta ascent 3
meta descent 1

glyph a 2 4
@@@@
@@@@
@@@@
@@@@

map A = a
",
        );
        let m = VMetrics::read(&font);
        assert_eq!((m.ascent, m.descent), (3.0 / 4.0, 1.0 / 4.0));
        assert_eq!(m.above(64.0), 48.0);
        assert_eq!(m.below(64.0), 16.0);
    }

    /// Ink that runs past the alignment box still has to be covered, or the
    /// preedit box would swallow it.
    #[test]
    fn read_widens_to_ink_outside_the_alignment_box() {
        let font = font_from(
            "\
meta height 4
meta ascent 3
meta descent 1

glyph tall 2 6 top -1
@@@@
@@@@
@@@@
@@@@
@@@@
@@@@

map A = tall
",
        );
        let m = VMetrics::read(&font);
        assert!(
            m.ascent > 3.0 / 4.0,
            "a glyph drawn above the ascent must widen the row, got {}",
            m.ascent,
        );
    }

    /// A face the preview cannot parse must not collapse the layout to
    /// nothing; the widget still has rows to draw.
    #[test]
    fn read_falls_back_when_the_face_is_unreadable() {
        assert_eq!(VMetrics::read(b"not a font"), VMetrics::default());
        assert_eq!(VMetrics::read(&[]), VMetrics::default());
    }

    /// Rows may not overlap: whatever rhythm the widget picks, a row still has
    /// to hold the face it draws.
    #[test]
    fn line_height_never_crops_the_face() {
        let tall = VMetrics {
            ascent: 1.6,
            descent: 0.6,
        };
        for px in SIZES {
            assert!(
                tall.line_height(px) >= tall.above(px) + tall.below(px),
                "a {px}px row of {}em cannot hold {}em of face",
                tall.line_height(px) / px,
                tall.ascent + tall.descent,
            );
            // Unison, whose box is exactly one em, keeps the airier rhythm.
            assert!(UNISON.line_height(px) > px);
        }
    }
}
