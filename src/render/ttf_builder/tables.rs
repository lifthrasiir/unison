//! Assembly of the final font: the non-layout OpenType tables.

use super::*;
use super::gpos::{build_anchor_gpos, merge_anchor_feature_lookups};
use super::gsub::{build_gsub, compute_max_context};
use super::outlines::{
    GlobalBounds, OutlineBuild, add_color_layer_glyphs, build_glyph_outlines,
    compute_global_bounds
};

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
    pixel_ascent: u16,
) -> Vec<u8> {
    let mut num_glyphs = u16::try_from(glyphs.len() + 1).expect("glyph count checked earlier"); // +1 for .notdef

    let default_aw = glyphs
        .iter()
        .find(|g| g.codepoint == Some(0x20))
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

    // head
    let head = Head {
        font_revision: Fixed::from_f64(1.0),
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
        line_gap: 0i16.into(),
        advance_width_max: aw_max.into(),
        min_left_side_bearing: min_lsb.into(),
        min_right_side_bearing: min_rsb.into(),
        x_max_extent: x_max_extent.into(),
        caret_slope_rise: 1,
        caret_slope_run: 0,
        caret_offset: 0,
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
    let name = build_name_table();

    // os2
    let avg_width = if glyphs.is_empty() {
        default_aw as i16
    } else {
        let total: u32 = glyphs.iter().map(|g| g.advance_width as u32).sum();
        (total / glyphs.len() as u32) as i16
    };
    let first_cp = glyphs.iter().filter_map(|g| g.codepoint).min().unwrap_or(0x20);
    let last_cp = glyphs.iter().filter_map(|g| g.codepoint).max().unwrap_or(0x7E);

    let max_context = compute_max_context(gsub_data);

    let os2 = Os2 {
        x_avg_char_width: avg_width,
        us_weight_class: 400,
        us_width_class: 5,
        s_typo_ascender: ascender,
        s_typo_descender: descender,
        s_typo_line_gap: 0,
        us_win_ascent: ascender as u16,
        us_win_descent: descender.unsigned_abs(),
        fs_selection: SelectionFlags::REGULAR,
        us_first_char_index: first_cp.min(0xFFFF) as u16,
        us_last_char_index: last_cp.min(0xFFFF) as u16,
        ach_vend_id: Tag::new(b"UNIF"),
        panose_10: [2, 0, 5, 9, 0, 0, 0, 0, 0, 0],
        sx_height: Some(ascender * 2 / 3),
        s_cap_height: Some(ascender),
        us_default_char: Some(0),
        us_break_char: Some(0x20),
        us_max_context: Some(max_context),
        ul_code_page_range_1: Some(1),
        ul_code_page_range_2: Some(0),
        ..Default::default()
    };

    // post
    let post = Post {
        version: write_fonts::types::Version16Dot16::new(3, 0),
        underline_position: (descender / 2).into(),
        underline_thickness: (UNITS_PER_EM as i16 / 20).into(),
        is_fixed_pitch: 1,
        ..Default::default()
    };

    let mut gsub = build_gsub(gsub_data, &name_to_gid);

    let mut anchor_data = build_anchor_gpos(
        glyphs,
        gsub_data,
        &name_to_gid,
        scale,
        pixel_ascent,
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

fn build_name_table() -> Name {
    let entries: &[(u16, &str)] = &[
        (0, "Uniform Font"),
        (1, "Uniform"),
        (2, "Regular"),
        (3, "Uniform-Regular"),
        (4, "Uniform Regular"),
        (5, "Version 1.0"),
        (6, "Uniform-Regular"),
    ];

    let records: Vec<NameRecord> = entries
        .iter()
        .map(|&(name_id, text)| {
            NameRecord::new(
                3, // Windows
                1, // Unicode BMP
                0x0409,
                NameId::new(name_id),
                String::from(text).into(),
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
