//! Assembly of the final font: the non-layout OpenType tables.

use super::*;
use super::gpos::{build_anchor_gpos, merge_anchor_feature_lookups};
use super::gsub::{build_gsub, compute_max_context};
use super::outlines::{
    GlobalBounds, OutlineBuild, add_color_layer_glyphs, build_glyph_outlines,
    compute_global_bounds
};

/// `head.created`/`modified` when `meta created` says nothing.
///
/// A fixed value rather than the wall clock, so that building the same source
/// twice produces the same bytes — a build that differs only in its timestamp
/// makes every diff of a built font useless. 1904-01-01 is the epoch itself,
/// which reads as "unstated" rather than as a wrong date.
const DEFAULT_CREATED: i64 = 0;

/// OpenType tag from a string, right-padded with spaces to 4 bytes.
pub(super) fn make_tag(name: &str) -> Tag {
    let mut tag_arr = [b' '; 4];
    for (i, &b) in name.as_bytes().iter().enumerate().take(4) {
        tag_arr[i] = b;
    }
    Tag::new(&tag_arr)
}

/// The `for` target of a `feature` directive: an OpenType script tag,
/// optionally narrowed to one language system under it.
///
/// The two registries are separate and their tags never collide as bytes —
/// script tags are lowercase and language tags uppercase — but the one pair
/// that looks like a collision is inverted (`DFLT` is the default *script*,
/// `dflt` the default *language*), and a language tag means nothing without
/// the script it hangs under (`SRB` exists below both `latn` and `cyrl`).
/// So the two are written explicitly as `script/LANG` rather than guessed
/// from the tag.
pub(super) fn parse_script_lang(tok: &str) -> (String, Option<String>) {
    match tok.split_once('/') {
        Some((script, lang)) => (script.to_string(), Some(lang.to_string())),
        None => (tok.to_string(), None),
    }
}

/// Feature indices for one script: the default language system plus any
/// explicit ones.
#[derive(Default)]
pub(super) struct ScriptFeatures {
    /// Features of the default LangSys, i.e. what a bare `for latn` declares.
    pub(super) default: Vec<u16>,
    /// Features of each explicit LangSys, keyed by language tag.
    pub(super) langs: BTreeMap<String, Vec<u16>>,
}

impl ScriptFeatures {
    pub(super) fn push(&mut self, lang: Option<&str>, feat_idx: u16) {
        match lang {
            None => self.default.push(feat_idx),
            Some(l) => self.langs.entry(l.to_string()).or_default().push(feat_idx),
        }
    }
}

/// Script records, each with its default LangSys and one record per explicit
/// language system.
///
/// `langs` is expected to be complete: a shaper that resolves a language picks
/// that LangSys *instead of* the default and never merges the two, so whatever
/// the default carries has to be repeated there. `gsub::build_gsub` does that
/// at feature-tag level, before the records exist, so a language that redefines
/// a tag the default already has ends up with one merged record rather than two
/// records the shaper would have to choose between.
pub(super) fn build_script_records(script_features: &BTreeMap<String, ScriptFeatures>) -> Vec<ScriptRecord> {
    let mut script_records: Vec<ScriptRecord> = Vec::new();
    for (script_tag, features) in script_features {
        let lang_sys = LangSys {
            required_feature_index: 0xFFFF,
            feature_indices: features.default.clone(),
        };
        let lang_sys_records: Vec<LangSysRecord> = features
            .langs
            .iter()
            .map(|(lang_tag, feat_indices)| {
                LangSysRecord::new(
                    make_tag(lang_tag),
                    LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: feat_indices.clone(),
                    },
                )
            })
            .collect();
        let script = Script::new(Some(lang_sys), lang_sys_records);
        script_records.push(ScriptRecord::new(make_tag(script_tag), script));
    }
    script_records
}

pub(super) fn build_ttf(
    ascender: i16,
    descender: i16,
    glyphs: &[CollectedGlyph],
    hint_ppem: u16,
    gsub_data: &GsubData,
    palette: &[Rgba],
    scale: f32,
    meta: &FontMeta,
) -> Vec<u8> {
    let mut num_glyphs = u16::try_from(glyphs.len() + 1).expect("glyph count checked earlier"); // +1 for .notdef

    let default_aw = glyphs
        .iter()
        .find(|g| g.codepoints.contains(&0x20))
        .or(glyphs.first())
        .map(|g| g.advance_width)
        .unwrap_or(UNITS_PER_EM / 2);

    let mut outlines = build_glyph_outlines(glyphs, hint_ppem, default_aw);
    let (colr_base_glyphs, colr_layers) =
        add_color_layer_glyphs(glyphs, &mut outlines, &mut num_glyphs);
    let has_color = !colr_base_glyphs.is_empty();
    let colr_layers_count = colr_layers.len() as u16;

    let OutlineBuild {
        glyf_builder,
        h_metrics,
        cmap_mappings,
        name_to_gid,
        max_points,
        max_contours,
        max_insn_size,
        max_stack,
        max_composite_points,
        max_composite_contours,
        max_component_elements,
        max_component_depth,
    } = outlines;

    let (glyf, loca, loca_format) = glyf_builder.build();

    let GlobalBounds {
        x_min,
        y_min,
        x_max,
        y_max,
        aw_max,
        min_lsb,
        min_rsb,
        x_max_extent,
    } = compute_global_bounds(glyphs, &h_metrics, default_aw, ascender, descender);

    // Every `meta` value on the pixel grid goes through the same scale the
    // outlines did, so a declaration reads in the units the source is drawn in.
    let px = |v: i16| (v as f32 * scale).round() as i16;
    let px_of = |key: crate::meta::PixelKey| meta.pixels(key).map(px);

    // head
    let head = Head {
        font_revision: Fixed::from_f64(meta.revision()),
        created: LongDateTime::new(meta.created.unwrap_or(DEFAULT_CREATED)),
        modified: LongDateTime::new(meta.created.unwrap_or(DEFAULT_CREATED)),
        mac_style: MacStyle::from_bits_truncate(meta.mac_style()),
        magic_number: 0x5F0F3CF5,
        flags: Flags::BASELINE_AT_Y_0
            | Flags::LSB_AT_X_0
            | Flags::INSTRUCTIONS_MAY_ALTER_ADVANCE_WIDTH,
        units_per_em: UNITS_PER_EM,
        x_min,
        y_min,
        x_max,
        y_max,
        lowest_rec_ppem: 8,
        font_direction_hint: 2,
        index_to_loc_format: loca_format as i16,
        ..Default::default()
    };

    // hhea
    let hhea = Hhea {
        ascender: ascender.into(),
        descender: descender.into(),
        line_gap: px_of(crate::meta::PixelKey::LineGap).unwrap_or(0).into(),
        advance_width_max: aw_max.into(),
        min_left_side_bearing: min_lsb.into(),
        min_right_side_bearing: min_rsb.into(),
        x_max_extent: x_max_extent.into(),
        caret_slope_rise: meta.caret_slope.map_or(1, |(rise, _)| rise),
        caret_slope_run: meta.caret_slope.map_or(0, |(_, run)| run),
        caret_offset: px_of(crate::meta::PixelKey::CaretOffset).unwrap_or(0),
        number_of_h_metrics: num_glyphs,
    };

    // maxp
    let maxp = Maxp {
        num_glyphs,
        max_points: Some(max_points),
        max_contours: Some(max_contours),
        max_composite_points: Some(max_composite_points),
        max_composite_contours: Some(max_composite_contours),
        max_zones: Some(1),
        max_twilight_points: Some(0),
        max_storage: Some(0),
        max_function_defs: Some(0),
        max_instruction_defs: Some(0),
        max_stack_elements: Some(max_stack),
        max_size_of_instructions: Some(max_insn_size),
        max_component_elements: Some(max_component_elements),
        max_component_depth: Some(max_component_depth),
    };

    // hmtx
    let hmtx = Hmtx {
        h_metrics,
        left_side_bearings: Vec::new(),
    };

    // cmap
    let cmap = Cmap::from_mappings(cmap_mappings).unwrap();

    // name
    let name = build_name_table(meta);

    // os2
    let avg_width = if glyphs.is_empty() {
        default_aw as i16
    } else {
        let total: u32 = glyphs.iter().map(|g| g.advance_width as u32).sum();
        (total / glyphs.len() as u32) as i16
    };
    let first_cp = glyphs.iter().flat_map(|g| &g.codepoints).min().copied().unwrap_or(0x20);
    let last_cp = glyphs.iter().flat_map(|g| &g.codepoints).max().copied().unwrap_or(0x7E);

    let max_context = compute_max_context(gsub_data);

    // Coverage-derived, never declared: these describe the font that came out.
    let mapped: std::collections::HashSet<u32> =
        glyphs.iter().flat_map(|g| &g.codepoints).copied().collect();
    let unicode_ranges = super::os2_ranges::unicode_ranges(mapped.iter().copied());
    let code_pages = super::os2_ranges::code_page_ranges(&mapped);

    // Sub/superscript and strikeout keep the conventional proportions of the em
    // when undeclared, rather than being left at zero.
    let em_frac = |num: i32| ((UNITS_PER_EM as i32) * num / 100) as i16;
    let default_sub_sup = [em_frac(65), em_frac(70), 0, em_frac(20)];
    let sub = meta.subscript.map(|v| v.map(px)).unwrap_or(default_sub_sup);
    let sup = meta
        .superscript
        .map(|v| v.map(px))
        .unwrap_or([default_sub_sup[0], default_sub_sup[1], 0, em_frac(48)]);
    let strikeout = meta
        .strikeout
        .map(|(size, pos)| (px(size), px(pos)))
        .unwrap_or((UNITS_PER_EM as i16 / 20, ascender / 3));

    let os2 = Os2 {
        x_avg_char_width: avg_width,
        us_weight_class: meta.weight.unwrap_or(400),
        us_width_class: meta.width.unwrap_or(5),
        fs_type: meta.fs_type.unwrap_or(0),
        s_typo_ascender: ascender,
        s_typo_descender: descender,
        s_typo_line_gap: px_of(crate::meta::PixelKey::LineGap).unwrap_or(0),
        us_win_ascent: ascender as u16,
        us_win_descent: descender.unsigned_abs(),
        fs_selection: SelectionFlags::from_bits_truncate(meta.fs_selection()),
        us_first_char_index: first_cp.min(0xFFFF) as u16,
        us_last_char_index: last_cp.min(0xFFFF) as u16,
        ach_vend_id: vendor_tag(meta.vendor_id()),
        panose_10: meta.panose.unwrap_or([2, 0, 5, 9, 0, 0, 0, 0, 0, 0]),
        sx_height: Some(px_of(crate::meta::PixelKey::XHeight).unwrap_or(ascender * 2 / 3)),
        s_cap_height: Some(px_of(crate::meta::PixelKey::CapHeight).unwrap_or(ascender)),
        y_subscript_x_size: sub[0],
        y_subscript_y_size: sub[1],
        y_subscript_x_offset: sub[2],
        y_subscript_y_offset: sub[3],
        y_superscript_x_size: sup[0],
        y_superscript_y_size: sup[1],
        y_superscript_x_offset: sup[2],
        y_superscript_y_offset: sup[3],
        y_strikeout_size: strikeout.0,
        y_strikeout_position: strikeout.1,
        us_default_char: Some(0),
        us_break_char: Some(0x20),
        us_max_context: Some(max_context),
        ul_unicode_range_1: unicode_ranges[0],
        ul_unicode_range_2: unicode_ranges[1],
        ul_unicode_range_3: unicode_ranges[2],
        ul_unicode_range_4: unicode_ranges[3],
        ul_code_page_range_1: Some(code_pages[0]),
        ul_code_page_range_2: Some(code_pages[1]),
        ..Default::default()
    };

    // post
    let post = Post {
        version: write_fonts::types::Version16Dot16::new(3, 0),
        underline_position: meta
            .underline
            .map_or(descender / 2, |(pos, _)| px(pos))
            .into(),
        underline_thickness: meta
            .underline
            .map_or(UNITS_PER_EM as i16 / 20, |(_, thick)| px(thick))
            .into(),
        is_fixed_pitch: u32::from(meta.has(crate::meta::StyleFlag::FixedPitch)),
        ..Default::default()
    };

    let mut gsub = build_gsub(gsub_data, &name_to_gid);

    let mut anchor_data = build_anchor_gpos(
        glyphs,
        gsub_data,
        &name_to_gid,
        scale,
        meta.ascent(),
    );
    merge_anchor_feature_lookups(&mut gsub, std::mem::take(&mut anchor_data.feature_lookups));

    let mut builder = FontBuilder::new();
    builder
        .add_table(&head)
        .unwrap()
        .add_table(&hhea)
        .unwrap()
        .add_table(&maxp)
        .unwrap()
        .add_table(&hmtx)
        .unwrap()
        .add_table(&cmap)
        .unwrap()
        .add_table(&name)
        .unwrap()
        .add_table(&os2)
        .unwrap()
        .add_table(&post)
        .unwrap()
        .add_table(&glyf)
        .unwrap()
        .add_table(&loca)
        .unwrap();

    let has_gsub = gsub.is_some();
    let has_gpos = anchor_data.gpos.is_some();

    if let Some(ref gsub) = gsub {
        builder.add_table(gsub).unwrap();
    }

    if let Some(ref gpos) = anchor_data.gpos {
        builder.add_table(gpos).unwrap();
    }

    if has_gsub || has_gpos {
        let mut gdef = anchor_data.gdef;
        if !anchor_data.mark_glyph_sets.is_empty() {
            gdef.mark_glyph_sets_def =
                Some(MarkGlyphSets::new(anchor_data.mark_glyph_sets)).into();
        }
        builder.add_table(&gdef).unwrap();
    }

    if has_color {
        let colr = Colr::new(
            colr_base_glyphs.len() as u16,
            Some(colr_base_glyphs),
            Some(colr_layers),
            colr_layers_count,
        );
        builder.add_table(&colr).unwrap();

        let color_records: Vec<ColorRecord> = palette
            .iter()
            .map(|c| ColorRecord::new(c.b, c.g, c.r, c.a))
            .collect();
        let num_entries = color_records.len() as u16;
        let cpal = Cpal::new(
            num_entries,
            1,
            num_entries,
            Some(color_records),
            vec![0],
        );
        builder.add_table(&cpal).unwrap();
    }

    builder.build()
}

/// `OS/2.achVendID` is a 4-byte tag; `meta` bounds the value to 1-4 printable
/// ASCII characters, so padding is all that is left to do here.
fn vendor_tag(id: &str) -> Tag {
    let mut bytes = [b' '; 4];
    for (i, &b) in id.as_bytes().iter().enumerate().take(4) {
        bytes[i] = b;
    }
    Tag::new(&bytes)
}

/// The name table, from what `meta` declares plus what [`FontMeta::name_records`]
/// derives.
///
/// Windows platform records only (platform 3, encoding 1). Platform 1 (Mac)
/// records are optional in practice and carry no language slot worth having,
/// and platform 0 records cannot be localized at all — so a `@LANG` on a `meta`
/// line would have nowhere to go outside platform 3.
fn build_name_table(meta: &FontMeta) -> Name {
    let records: Vec<NameRecord> = meta
        .name_records()
        .into_iter()
        .map(|(name_id, language_id, text)| {
            NameRecord::new(
                3, // Windows
                1, // Unicode BMP
                language_id,
                NameId::new(name_id),
                text.into(),
            )
        })
        .collect();

    Name::new(records)
}

pub(super) fn glyph_bounds(contours: &[Vec<(i16, i16)>]) -> (i16, i16, i16, i16) {
    let mut x_min = i16::MAX;
    let mut y_min = i16::MAX;
    let mut x_max = i16::MIN;
    let mut y_max = i16::MIN;
    for c in contours {
        for &(x, y) in c {
            x_min = x_min.min(x);
            y_min = y_min.min(y);
            x_max = x_max.max(x);
            y_max = y_max.max(y);
        }
    }
    if x_min > x_max {
        return (0, 0, 0, 0);
    }
    (x_min, y_min, x_max, y_max)
}
