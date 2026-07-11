use crate::preview::{Feature, ShapeError, ShapedGlyph, ShaperBackend};

pub struct HarfBuzzBackend;

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

impl ShaperBackend for HarfBuzzBackend {
    fn name(&self) -> &'static str {
        "HarfBuzz"
    }

    fn shape(
        &self,
        font_data: &[u8],
        text: &str,
        _upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError> {
        let face = harfbuzz_rs::Face::from_bytes(font_data, 0);
        let font = harfbuzz_rs::Font::new(face);

        let buffer = harfbuzz_rs::UnicodeBuffer::new().add_str(text);

        let hb_features: Vec<harfbuzz_rs::Feature> = features
            .iter()
            .map(|f| {
                let tag = harfbuzz_rs::Tag::new(
                    f.tag[0] as char,
                    f.tag[1] as char,
                    f.tag[2] as char,
                    f.tag[3] as char,
                );
                harfbuzz_rs::Feature::new(tag, f.value, f.start..f.end)
            })
            .collect();

        let output = harfbuzz_rs::shape(&font, buffer, &hb_features);

        let positions = output.get_glyph_positions();
        let infos = output.get_glyph_infos();

        let byte_to_char = byte_to_char_map(text);

        let upem = font.face().upem() as f32;
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
                    glyph_id: info.codepoint as u16,
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
