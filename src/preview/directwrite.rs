//! The DirectWrite backend.
//!
//! Unlike Core Text's `CTLine`, this sits on `IDWriteTextAnalyzer` — the
//! *analysis* layer, below layout — so nothing here reorders anything on its
//! own. The analyzer does expose `AnalyzeBidi`, and switching to it is what
//! would make this backend proof DirectWrite's own UAX #9 the way the Core Text
//! one proofs Core Text's; until then it takes the shared resolution
//! ([`crate::preview::shape_runs`]) and only tells `GetGlyphs` which direction
//! each run is, through `isRightToLeft`.
//!
//! Whether that flag also applies rule L4 — DirectWrite's documentation does
//! not say whether `GetGlyphs` substitutes the `Bidi_Mirroring_Glyph` the way
//! HarfBuzz does — is the thing to check on a real Windows run; if it does not,
//! the code point swap has to happen here before the call.

use crate::preview::{Feature, Paragraph, ShapeError, ShapedGlyph, ShaperBackend, shape_runs};

pub struct DirectWriteBackend;

impl ShaperBackend for DirectWriteBackend {
    fn name(&self) -> &'static str {
        "DirectWrite"
    }

    fn shape(
        &self,
        font_data: &[u8],
        para: &Paragraph<'_>,
        upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError> {
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::Graphics::DirectWrite::*;
        use windows::core::PCWSTR;

        unsafe {
            let factory: IDWriteFactory5 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(err)?;

            let font_file_loader: IDWriteInMemoryFontFileLoader =
                factory.CreateInMemoryFontFileLoader().map_err(err)?;

            factory
                .RegisterFontFileLoader(&font_file_loader)
                .map_err(err)?;

            let font_file = font_file_loader
                .CreateInMemoryFontFileReference(
                    &factory,
                    font_data.as_ptr() as *const _,
                    font_data.len() as u32,
                    None,
                )
                .map_err(err)?;

            let face: IDWriteFontFace = factory
                .CreateFontFace(
                    DWRITE_FONT_FACE_TYPE_TRUETYPE,
                    &[Some(font_file)],
                    0,
                    DWRITE_FONT_SIMULATIONS_NONE,
                )
                .map_err(err)?;

            let analyzer: IDWriteTextAnalyzer = factory.CreateTextAnalyzer().map_err(err)?;

            // The feature values live in their own Vec so the pointers stored
            // in `dw_features` stay valid for the GetGlyphs/GetGlyphPlacements
            // calls below; a `&DWRITE_FONT_FEATURE { .. }` temporary inside the
            // map would be freed as soon as the closure returned.
            let feature_values: Vec<DWRITE_FONT_FEATURE> = features
                .iter()
                .map(|f| DWRITE_FONT_FEATURE {
                    nameTag: DWRITE_FONT_FEATURE_TAG(u32::from_le_bytes(f.tag)),
                    parameter: f.value,
                })
                .collect();
            let dw_features: Vec<DWRITE_TYPOGRAPHIC_FEATURES> = feature_values
                .iter()
                .map(|v| DWRITE_TYPOGRAPHIC_FEATURES {
                    features: v as *const _ as *mut _,
                    featureCount: 1,
                })
                .collect();

            let shaped = shape_runs(para, features, |text, level, _run_features| {
                let rtl = level % 2 == 1;
                let utf16: Vec<u16> = text.encode_utf16().collect();
                let text_len = utf16.len() as u32;
                if text_len == 0 {
                    return Ok(Vec::new());
                }

                // The run is already one script by `crate::script_run`, but the
                // analyzer is what produces the `DWRITE_SCRIPT_ANALYSIS` the
                // shaping calls need, so it still runs over it. Should it split
                // the run further, a backward one is walked back to front so
                // the glyphs still come out in visual order.
                let source = TextAnalysisSource::new(&utf16, rtl);
                let sink = TextAnalysisSink::new();
                analyzer
                    .AnalyzeScript(&source, 0, text_len, &sink)
                    .map_err(err)?;

                let sink_impl: &TextAnalysisSink = sink.as_impl();
                let mut script_runs: Vec<ScriptRun> = sink_impl
                    .runs
                    .borrow()
                    .iter()
                    .map(|r| ScriptRun {
                        start: r.start,
                        len: r.len,
                        analysis: r.analysis,
                    })
                    .collect();
                if rtl {
                    script_runs.reverse();
                }

                let utf16_to_char = build_utf16_to_char_map(text);
                let mut all_glyphs = Vec::new();

                for run in &script_runs {
                let start = run.start as usize;
                let len = run.len as usize;
                let sub_text = &utf16[start..start + len];

                let max_glyphs = (len as u32 * 3 / 2 + 16).max(len as u32 + 1);
                let mut cluster_map = vec![0u16; len];
                let mut text_props = vec![DWRITE_SHAPING_TEXT_PROPERTIES::default(); len];
                let mut glyph_indices = vec![0u16; max_glyphs as usize];
                let mut glyph_props =
                    vec![DWRITE_SHAPING_GLYPH_PROPERTIES::default(); max_glyphs as usize];
                let mut actual_glyph_count = 0u32;

                let feature_ptrs: Vec<*const DWRITE_TYPOGRAPHIC_FEATURES> =
                    dw_features.iter().map(|f| f as *const _).collect();
                let feature_range_lengths: Vec<u32> = vec![len as u32; dw_features.len()];

                let (feat_opt, feat_range_opt, feat_count) = if dw_features.is_empty() {
                    (None, None, 0u32)
                } else {
                    (
                        Some(feature_ptrs.as_ptr() as *const *const DWRITE_TYPOGRAPHIC_FEATURES),
                        Some(feature_range_lengths.as_ptr()),
                        dw_features.len() as u32,
                    )
                };

                analyzer
                    .GetGlyphs(
                        PCWSTR(sub_text.as_ptr()),
                        len as u32,
                        &face,
                        // isSideways, isRightToLeft
                        BOOL::from(false),
                        BOOL::from(rtl),
                        &run.analysis,
                        PCWSTR::null(),
                        None,
                        feat_opt,
                        feat_range_opt,
                        feat_count,
                        max_glyphs,
                        cluster_map.as_mut_ptr(),
                        text_props.as_mut_ptr(),
                        glyph_indices.as_mut_ptr(),
                        glyph_props.as_mut_ptr(),
                        &mut actual_glyph_count,
                    )
                    .map_err(err)?;

                let glyph_count = actual_glyph_count as usize;
                glyph_indices.truncate(glyph_count);
                glyph_props.truncate(glyph_count);

                let mut advances = vec![0.0f32; glyph_count];
                let mut offsets = vec![DWRITE_GLYPH_OFFSET::default(); glyph_count];

                analyzer
                    .GetGlyphPlacements(
                        PCWSTR(sub_text.as_ptr()),
                        cluster_map.as_ptr(),
                        text_props.as_mut_ptr(),
                        len as u32,
                        glyph_indices.as_ptr(),
                        glyph_props.as_ptr(),
                        glyph_count as u32,
                        &face,
                        upm as f32,
                        // isSideways, isRightToLeft
                        BOOL::from(false),
                        BOOL::from(rtl),
                        &run.analysis,
                        PCWSTR::null(),
                        feat_opt,
                        feat_range_opt,
                        feat_count,
                        advances.as_mut_ptr(),
                        offsets.as_mut_ptr(),
                    )
                    .map_err(err)?;

                for i in 0..glyph_count {
                    let cluster_utf16 = cluster_map
                        .iter()
                        .position(|&c| c as usize == i)
                        .map(|pos| start + pos)
                        .unwrap_or(start);

                    let cluster = if cluster_utf16 < utf16_to_char.len() {
                        utf16_to_char[cluster_utf16]
                    } else {
                        text.chars().count()
                    };

                    all_glyphs.push(ShapedGlyph {
                        glyph_id: glyph_indices[i],
                        cluster,
                        // `shape_runs` overwrites this with the run's level.
                        level,
                        x_advance: advances[i] / upm as f32,
                        y_advance: 0.0,
                        x_offset: offsets[i].advanceOffset / upm as f32,
                        y_offset: offsets[i].ascenderOffset / upm as f32,
                    });
                }
                }

                Ok(all_glyphs)
            });

            factory.UnregisterFontFileLoader(&font_file_loader).ok();

            shaped
        }
    }
}

fn err(e: windows::core::Error) -> ShapeError {
    ShapeError(format!("DirectWrite error: {e}"))
}

use super::build_utf16_to_char_map;

// --- COM implementations ---

struct ScriptRun {
    start: u32,
    len: u32,
    analysis: windows::Win32::Graphics::DirectWrite::DWRITE_SCRIPT_ANALYSIS,
}

use std::cell::RefCell;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::implement;
use windows_core::AsImpl;

#[implement(IDWriteTextAnalysisSource)]
struct TextAnalysisSource {
    text: *const u16,
    text_len: u32,
    rtl: bool,
}

impl TextAnalysisSource {
    fn new(text: &[u16], rtl: bool) -> IDWriteTextAnalysisSource {
        let source = TextAnalysisSource {
            text: text.as_ptr(),
            text_len: text.len() as u32,
            rtl,
        };
        source.into()
    }
}

impl IDWriteTextAnalysisSource_Impl for TextAnalysisSource_Impl {
    fn GetTextAtPosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        unsafe {
            if textposition >= self.text_len {
                *textstring = std::ptr::null_mut();
                *textlength = 0;
            } else {
                *textstring = self.text.add(textposition as usize) as *mut u16;
                *textlength = self.text_len - textposition;
            }
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        unsafe {
            if textposition == 0 || textposition > self.text_len {
                *textstring = std::ptr::null_mut();
                *textlength = 0;
            } else {
                *textstring = self.text as *mut u16;
                *textlength = textposition;
            }
        }
        Ok(())
    }

    /// The run's own direction, not the paragraph's: this source is handed one
    /// single-direction run at a time, and `AnalyzeScript` uses this to decide
    /// which way a run of neutrals in it reads.
    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        if self.rtl {
            DWRITE_READING_DIRECTION_RIGHT_TO_LEFT
        } else {
            DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
        }
    }

    fn GetLocaleName(
        &self,
        _textposition: u32,
        textlength: *mut u32,
        localename: *mut *mut u16,
    ) -> windows::core::Result<()> {
        unsafe {
            *textlength = self.text_len;
            static EMPTY: u16 = 0;
            *localename = &EMPTY as *const u16 as *mut u16;
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        _textposition: u32,
        textlength: *mut u32,
        numbersubstitution: *mut Option<IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        unsafe {
            *textlength = self.text_len;
            *numbersubstitution = None;
        }
        Ok(())
    }
}

#[implement(IDWriteTextAnalysisSink)]
struct TextAnalysisSink {
    runs: RefCell<Vec<ScriptRun>>,
}

impl TextAnalysisSink {
    fn new() -> IDWriteTextAnalysisSink {
        let sink = TextAnalysisSink {
            runs: RefCell::new(Vec::new()),
        };
        sink.into()
    }
}

impl IDWriteTextAnalysisSink_Impl for TextAnalysisSink_Impl {
    fn SetScriptAnalysis(
        &self,
        textposition: u32,
        textlength: u32,
        scriptanalysis: *const DWRITE_SCRIPT_ANALYSIS,
    ) -> windows::core::Result<()> {
        unsafe {
            self.runs.borrow_mut().push(ScriptRun {
                start: textposition,
                len: textlength,
                analysis: *scriptanalysis,
            });
        }
        Ok(())
    }

    fn SetLineBreakpoints(
        &self,
        _textposition: u32,
        _textlength: u32,
        _linebreakpoints: *const DWRITE_LINE_BREAKPOINT,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetBidiLevel(
        &self,
        _textposition: u32,
        _textlength: u32,
        _explicitlevel: u8,
        _resolvedlevel: u8,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetNumberSubstitution(
        &self,
        _textposition: u32,
        _textlength: u32,
        _numbersubstitution: Option<&IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}
