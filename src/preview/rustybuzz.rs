//! The rustybuzz backend: a bare shaper, so everything above it is ours.
//!
//! Unlike Core Text and DirectWrite this is a shaper and nothing more — it has
//! no notion of a paragraph, a direction or a script boundary — so this backend
//! is the one that runs [`crate::preview::shape_runs`] over the resolved bidi
//! runs. Mirroring (UAX #9 rule L4) it *does* do on its own: for a backward run
//! rustybuzz swaps in the `Bidi_Mirroring_Glyph` code point when the face has a
//! glyph for it, and otherwise sets the `rtlm` mask so the font's own feature
//! can. Nothing here has to arrange that.
//!
//! This is the backend that stands in for what a browser does, since a browser
//! is UAX #9 over HarfBuzz too.

use crate::preview::{Feature, Paragraph, ShapeError, ShapedGlyph, ShaperBackend, shape_runs};
use crate::render::assert::feature_tag;

pub struct RustyBuzzBackend;

fn byte_to_char_map(text: &str) -> Vec<usize> {
    let mut map = vec![0usize; text.len() + 1];
    for (char_idx, (byte_idx, _)) in text.char_indices().enumerate() {
        map[byte_idx] = char_idx;
    }
    map[text.len()] = text.chars().count();
    // fill gaps (multi-byte chars) by propagating forward
    let mut last = 0;
    for v in &mut map {
        if *v != 0 {
            last = *v;
        } else {
            *v = last;
        }
    }
    map
}

impl ShaperBackend for RustyBuzzBackend {
    fn name(&self) -> &'static str {
        "rustybuzz"
    }

    fn shape(
        &self,
        font_data: &[u8],
        para: &Paragraph<'_>,
        upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError> {
        let face = rustybuzz::Face::from_slice(font_data, 0)
            .ok_or_else(|| ShapeError("failed to parse the font".to_string()))?;
        shape_runs(para, features, |text, level, run_features| {
            Ok(shape_one_run(&face, text, level, upm, run_features))
        })
    }
}

/// Shape one single-direction, single-script run. The glyphs come back in
/// visual order, which for a backward run means rustybuzz has already reversed
/// them.
fn shape_one_run(
    face: &rustybuzz::Face<'_>,
    text: &str,
    level: u8,
    upm: u16,
    features: &[Feature],
) -> Vec<ShapedGlyph> {
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    // Set rather than guessed: the level is the resolved one, and a run of
    // neutrals inside a right-to-left paragraph has no strong character for
    // `guess_segment_properties` to have picked it from.
    buffer.set_direction(if level % 2 == 1 {
        rustybuzz::Direction::RightToLeft
    } else {
        rustybuzz::Direction::LeftToRight
    });

    let hb_features: Vec<rustybuzz::Feature> = features
        .iter()
        .map(|f| rustybuzz::Feature::new(feature_tag(&f.tag), f.value, f.start..f.end))
        .collect();

    let output = rustybuzz::shape(face, &hb_features, buffer);

    let positions = output.glyph_positions();
    let infos = output.glyph_infos();
    let byte_to_char = byte_to_char_map(text);
    let upem = upm as f32;

    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, pos)| {
            let cluster_byte = info.cluster as usize;
            let cluster = if cluster_byte < byte_to_char.len() {
                byte_to_char[cluster_byte]
            } else {
                text.chars().count()
            };
            ShapedGlyph {
                glyph_id: info.glyph_id as u16,
                cluster,
                level,
                x_advance: pos.x_advance as f32 / upem,
                y_advance: pos.y_advance as f32 / upem,
                x_offset: pos.x_offset as f32 / upem,
                y_offset: pos.y_offset as f32 / upem,
            }
        })
        .collect()
}
