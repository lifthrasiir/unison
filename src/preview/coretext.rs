use crate::preview::{Feature, ShapeError, ShapedGlyph, ShaperBackend};

pub struct CoreTextBackend;

impl ShaperBackend for CoreTextBackend {
    fn name(&self) -> &'static str {
        "Core Text"
    }

    fn shape(
        &self,
        font_data: &[u8],
        text: &str,
        upm: u16,
        _features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError> {
        use core_foundation::attributed_string::CFMutableAttributedString;
        use core_foundation::base::{CFRange, TCFType};
        use core_foundation::string::CFString;
        use core_graphics::data_provider::CGDataProvider;
        use core_graphics::font::CGFont;
        use core_text::font as ct_font;
        use core_text::string_attributes::kCTFontAttributeName;

        let provider = CGDataProvider::from_buffer(std::sync::Arc::new(font_data.to_vec()));
        let cg_font = CGFont::from_data_provider(provider)
            .map_err(|_| ShapeError("Failed to create CGFont".into()))?;

        let pt_size = upm as f64;
        let ct_font = ct_font::new_from_CGFont(&cg_font, pt_size);

        let cf_string = CFString::new(text);
        let mut attr_string = CFMutableAttributedString::new();
        attr_string.replace_str(
            &cf_string,
            CFRange {
                location: 0,
                length: 0,
            },
        );
        let len = attr_string.char_len();
        unsafe {
            attr_string.set_attribute(
                CFRange {
                    location: 0,
                    length: len,
                },
                kCTFontAttributeName,
                &ct_font,
            );
        }

        let line = core_text::line::CTLine::new_with_attributed_string(
            attr_string.as_concrete_TypeRef() as _,
        );

        let utf16_to_char = build_utf16_to_char_map(text);
        let total_chars = text.chars().count();

        let mut result = Vec::new();
        let runs = line.glyph_runs();

        for run in runs.iter() {
            let glyph_count = run.glyph_count() as usize;
            if glyph_count == 0 {
                continue;
            }

            let glyphs = run.glyphs();
            let positions = run.positions();
            let string_indices = run.string_indices();

            for j in 0..glyph_count {
                let glyph_id = glyphs[j];
                let utf16_idx = string_indices[j] as usize;
                let cluster = if utf16_idx < utf16_to_char.len() {
                    utf16_to_char[utf16_idx]
                } else {
                    total_chars
                };

                let x_advance = if j + 1 < glyph_count {
                    (positions[j + 1].x - positions[j].x) as f32 / upm as f32
                } else {
                    // last glyph: use line typographic bounds
                    let bounds = line.get_typographic_bounds();
                    let total_width = bounds.width as f32;
                    (total_width - positions[j].x as f32) / upm as f32
                };

                result.push(ShapedGlyph {
                    glyph_id,
                    cluster,
                    x_advance,
                    y_advance: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                });
            }
        }

        Ok(result)
    }
}

use super::build_utf16_to_char_map;
