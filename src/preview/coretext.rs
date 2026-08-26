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
//! # Font fallback, and why it must not reach the rasterizer
//!
//! `CTLine` is a layout object, so it does what a layout object does with a
//! character the font cannot draw: it splits the run off and *substitutes
//! another font* for it. The glyph ids that come back are then indices into
//! that other font, and the preview rasterizes every glyph id it is given
//! against the built one — so an undrawable character came out as whatever
//! Unison happens to have at that id, which for Hebrew was a scatter of Latin
//! Extended-A. A proofing tool showing a plausible glyph where the font has
//! nothing is worse than useless, so any run that comes back set in another
//! font is replaced with `.notdef`, which is the honest answer.
//!
//! Stopping the substitution at its source does not work, and was tried: an
//! empty `kCTFontCascadeListAttribute` on the font changes nothing, because
//! `CTLine` substitutes at the *layout* level rather than through the font's
//! own cascade list. The test below pins that — it still failed with the
//! cascade list in place. What the run then keeps is the substitute font's
//! advances, since the positions are Core Text's; the glyph is ours.
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
        use core_text::string_attributes::kCTFontAttributeName;

        let text = para.text;
        let provider = CGDataProvider::from_buffer(std::sync::Arc::new(font_data.to_vec()));
        let cg_font = CGFont::from_data_provider(provider)
            .map_err(|_| ShapeError("Failed to create CGFont".into()))?;

        let pt_size = upm as f64;
        let ct_font = core_text::font::new_from_CGFont(&cg_font, pt_size);
        let own_name = ct_font.postscript_name();

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
            // A run Core Text substituted a font into carries that font's glyph
            // ids, which mean nothing here; `.notdef` is what the built font
            // actually has for those characters.
            let substituted = !run_is_font(&run, &own_name);
            let glyphs = run.glyphs();
            let positions = run.positions();
            let string_indices = run.string_indices();
            for j in 0..glyph_count {
                let utf16_idx = string_indices[j] as usize;
                let cluster = utf16_to_char
                    .get(utf16_idx)
                    .copied()
                    .unwrap_or(total_chars);
                let glyph_id = if substituted { 0 } else { glyphs[j] };
                placed.push((glyph_id, cluster, positions[j].x as f32));
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

/// Whether a run is still set in the font we asked for, rather than one Core
/// Text substituted. Compared by PostScript name: the run's font is a separate
/// instance even when it is the same face, so identity says nothing.
fn run_is_font(run: &core_text::run::CTRun, own_name: &str) -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_text::string_attributes::kCTFontAttributeName;

    let Some(attributes) = run.attributes() else {
        return true;
    };
    // SAFETY: reading one of Core Text's own attribute-name constants.
    let key = unsafe { CFString::wrap_under_get_rule(kCTFontAttributeName) };
    let Some(value) = attributes.find(&key) else {
        return true;
    };
    match value.downcast::<core_text::font::CTFont>() {
        Some(font) => font.postscript_name() == own_name,
        None => true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io;
    use crate::preview::bidi::ParagraphDirection;
    use crate::render::ttf_builder::build_font_with_gid_map;

    /// A font with exactly one mapped character, so anything else in the text
    /// is something it cannot draw.
    fn one_glyph_font() -> Vec<u8> {
        let input = "\
glyph pix 1 1
@@
glyph a
ref pix
map A = a
";
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        build_font_with_gid_map(&[&doc])
            .expect("font should build")
            .ttf
    }

    fn glyph_ids(text: &str) -> Vec<u16> {
        let font = one_glyph_font();
        let para = Paragraph::new(text, ParagraphDirection::Auto);
        CoreTextBackend
            .shape(&font, &para, 1024, &[])
            .expect("Core Text should shape")
            .iter()
            .map(|g| g.glyph_id)
            .collect()
    }

    #[test]
    fn a_character_the_font_covers_is_drawn_from_it() {
        assert_eq!(glyph_ids("A"), vec![1]);
    }

    /// The regression this exists for: Core Text substitutes another font for a
    /// character this one has no glyph for, and the ids it hands back index
    /// into *that* font. Rasterized against the built font they came out as
    /// unrelated glyphs — a font the preview cannot draw Hebrew with appeared
    /// to draw it. `.notdef` is the answer, whether the empty cascade list or
    /// the per-run check is what produced it.
    #[test]
    fn a_character_the_font_lacks_never_borrows_another_fonts_glyph_id() {
        // U+05D0 HEBREW LETTER ALEF, which `one_glyph_font` does not map.
        assert_eq!(glyph_ids("\u{05D0}"), vec![0]);
        // ...and mixed, where only the uncovered half is replaced.
        assert_eq!(glyph_ids("A\u{05D0}"), vec![1, 0]);
    }
}
