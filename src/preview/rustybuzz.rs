use crate::preview::{Feature, ShapeError, ShapedGlyph, ShaperBackend};
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
        text: &str,
        _upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError> {
        let face = rustybuzz::Face::from_slice(font_data, 0)
            .ok_or_else(|| ShapeError("failed to parse the font".to_string()))?;

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);

        let hb_features: Vec<rustybuzz::Feature> = features
            .iter()
            .map(|f| rustybuzz::Feature::new(feature_tag(&f.tag), f.value, f.start..f.end))
            .collect();

        let output = rustybuzz::shape(&face, &hb_features, buffer);

        let positions = output.glyph_positions();
        let infos = output.glyph_infos();

        let byte_to_char = byte_to_char_map(text);

        let upem = face.units_per_em() as f32;
        let result = infos
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
                    x_advance: pos.x_advance as f32 / upem,
                    y_advance: pos.y_advance as f32 / upem,
                    x_offset: pos.x_offset as f32 / upem,
                    y_offset: pos.y_offset as f32 / upem,
                }
            })
            .collect();

        Ok(result)
    }
}
