//! The Core Text backend: the platform stack macOS apps actually see.
//!
//! `CTLine` is a *layout* object, not a shaper: handed the whole paragraph it
//! runs UAX #9 itself — levels, reordering and rule L4 mirroring — and returns
//! its `CTRun`s already in visual order. So this backend hands it the text
//! whole and does none of that, which is the point: the preview exists to show
//! what this engine does with the built font, and resolving the bidi here
//! instead would replace the part being proofed. What it does take from the
//! shared path is [`Paragraph::char_levels`], purely to *label* the glyphs it
//! gets back, so the caret code sees one vocabulary across backends.
//!
//! # Forcing a direction
//!
//! `CTLine` picks the paragraph level by P2/P3 on its own, and there is no
//! argument for overriding it — the override is an attribute on the string: a
//! `CTParagraphStyle` carrying `kCTParagraphStyleSpecifierBaseWritingDirection`.
//! `core-text` does not wrap paragraph styles, so the few functions involved
//! are declared here.

use crate::preview::bidi::ParagraphDirection;
use crate::preview::{Feature, Paragraph, ShapeError, ShapedGlyph, ShaperBackend};

pub struct CoreTextBackend;

impl ShaperBackend for CoreTextBackend {
    fn name(&self) -> &'static str {
        "Core Text"
    }

    fn shape(
        &self,
        font_data: &[u8],
        para: &Paragraph<'_>,
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

        let text = para.text;
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
        let whole = CFRange {
            location: 0,
            length: len,
        };
        unsafe {
            attr_string.set_attribute(whole, kCTFontAttributeName, &ct_font);
        }
        if let Some(style) = paragraph_style_for(para.direction) {
            unsafe {
                attr_string.set_attribute(whole, kCTParagraphStyleAttributeName, &style);
            }
        }

        let line = core_text::line::CTLine::new_with_attributed_string(
            attr_string.as_concrete_TypeRef() as _,
        );

        let utf16_to_char = build_utf16_to_char_map(text);
        let total_chars = text.chars().count();

        // `CTRun` positions are relative to the *line* origin and the runs come
        // back in visual order, so the line reads as one left-to-right sequence
        // of pen positions. Advances are the gaps in that sequence: taking them
        // per run instead would make every run's last glyph reach to the end of
        // the whole line.
        let mut placed: Vec<(u16, usize, f32)> = Vec::new();
        for run in line.glyph_runs().iter() {
            let glyph_count = run.glyph_count() as usize;
            if glyph_count == 0 {
                continue;
            }
            let glyphs = run.glyphs();
            let positions = run.positions();
            let string_indices = run.string_indices();
            for j in 0..glyph_count {
                let utf16_idx = string_indices[j] as usize;
                let cluster = utf16_to_char
                    .get(utf16_idx)
                    .copied()
                    .unwrap_or(total_chars);
                placed.push((glyphs[j], cluster, positions[j].x as f32));
            }
        }

        let line_width = line.get_typographic_bounds().width as f32;
        let upem = upm as f32;
        let result = placed
            .iter()
            .enumerate()
            .map(|(i, &(glyph_id, cluster, x))| {
                let next_x = placed.get(i + 1).map_or(line_width, |p| p.2);
                ShapedGlyph {
                    glyph_id,
                    cluster,
                    level: para.level_of_char(cluster),
                    x_advance: (next_x - x) / upem,
                    y_advance: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                }
            })
            .collect();

        Ok(result)
    }
}

/// A paragraph style pinning the base writing direction, or `None` when the
/// direction is [`ParagraphDirection::Auto`] and Core Text's own P2/P3 is what
/// we want to see.
fn paragraph_style_for(
    direction: ParagraphDirection,
) -> Option<core_foundation::base::CFType> {
    use core_foundation::base::TCFType;

    let writing_direction: i8 = match direction {
        ParagraphDirection::Auto => return None,
        ParagraphDirection::Ltr => K_CT_WRITING_DIRECTION_LEFT_TO_RIGHT,
        ParagraphDirection::Rtl => K_CT_WRITING_DIRECTION_RIGHT_TO_LEFT,
    };
    let setting = CTParagraphStyleSetting {
        spec: K_CT_PARAGRAPH_STYLE_SPECIFIER_BASE_WRITING_DIRECTION,
        value_size: std::mem::size_of::<i8>(),
        value: (&raw const writing_direction).cast(),
    };
    // SAFETY: one setting, described by `setting`, whose `value` points at a
    // live `i8` of the size the struct claims. `CTParagraphStyleCreate` copies
    // what it is given, so the borrow ends with the call.
    unsafe {
        let style = CTParagraphStyleCreate(&raw const setting, 1);
        (!style.is_null()).then(|| core_foundation::base::CFType::wrap_under_create_rule(style))
    }
}

/// `kCTParagraphStyleSpecifierBaseWritingDirection`, from `CTParagraphStyle.h`.
const K_CT_PARAGRAPH_STYLE_SPECIFIER_BASE_WRITING_DIRECTION: u32 = 13;
/// `kCTWritingDirectionLeftToRight` / `…RightToLeft`, same header. (Natural is
/// `-1`, which is what leaving the attribute off already means.)
const K_CT_WRITING_DIRECTION_LEFT_TO_RIGHT: i8 = 0;
const K_CT_WRITING_DIRECTION_RIGHT_TO_LEFT: i8 = 1;

#[repr(C)]
struct CTParagraphStyleSetting {
    spec: u32,
    value_size: usize,
    value: *const std::ffi::c_void,
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTParagraphStyleCreate(
        settings: *const CTParagraphStyleSetting,
        setting_count: usize,
    ) -> core_foundation::base::CFTypeRef;
    static kCTParagraphStyleAttributeName: core_foundation::string::CFStringRef;
}

use super::build_utf16_to_char_map;
